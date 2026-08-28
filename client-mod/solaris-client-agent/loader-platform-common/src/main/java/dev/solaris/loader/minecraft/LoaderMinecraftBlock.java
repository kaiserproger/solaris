package dev.solaris.loader.minecraft;

import dev.solaris.loader.LoaderActivatedContent;
import dev.solaris.loader.LoaderBlockDefinition;
import dev.solaris.loader.LoaderScreenDefinition;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.network.chat.Component;
import net.minecraft.resources.Identifier;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;

public final class LoaderMinecraftBlock {
    private LoaderMinecraftBlock() {
    }

    public static Optional<ItemStack> forScreen(
            LoaderScreenDefinition screen,
            LoaderActivatedContent content) {
        Optional<String> blockId = screen.blockId();
        return blockId.flatMap(id -> resolve(screen, content)
                .flatMap(definition -> create(
                        definition,
                        carrierIndex(content, id))));
    }

    static Optional<LoaderBlockDefinition> resolve(
            LoaderScreenDefinition screen,
            LoaderActivatedContent content) {
        return screen.blockId()
                .map(content.blocks()::get)
                .filter(Objects::nonNull);
    }

    static void validate(LoaderActivatedContent content) {
        if (content.blocks().isEmpty()) {
            return;
        }
        if (content.blocks().size() > LoaderBlockCarrier.MAX_CARRIERS) {
            throw new IllegalStateException("Solaris Loader block carrier capacity exceeded");
        }
        // Runtime block-state ids are platform-owned. Fabric/NeoForge use the vanilla
        // state registry, while Forge 26.1.2 binds custom states through GameData's
        // block-state id map. The platform-specific ACK supplier performs the exact
        // state-id round-trip check after resource activation.
    }

    public static List<Integer> carrierStateIds() {
        List<Integer> stateIds = new ArrayList<>(LoaderBlockCarrier.MAX_CARRIERS);
        for (int index = 0; index < LoaderBlockCarrier.MAX_CARRIERS; index++) {
            Identifier id = LoaderBlockCarrier.id(index);
            if (!BuiltInRegistries.BLOCK.containsKey(id)
                    || !BuiltInRegistries.ITEM.containsKey(id)) {
                throw new IllegalStateException(
                        "Solaris Loader block carrier was not registered before registry freeze");
            }
            Block block = BuiltInRegistries.BLOCK.getValue(id);
            BlockState state = block.defaultBlockState();
            int stateId = Block.getId(state);
            BlockState roundTrip = stateId < 0 ? null : Block.BLOCK_STATE_REGISTRY.byId(stateId);
            if (stateId < 0 || roundTrip != state) {
                throw new IllegalStateException(
                        "Solaris Loader block carrier has no exact runtime state id: carrier="
                                + id
                                + ", block_registry_id=" + BuiltInRegistries.BLOCK.getId(block)
                                + ", state_id=" + stateId
                                + ", state_registry_size=" + Block.BLOCK_STATE_REGISTRY.size()
                                + ", roundtrip_same=" + (roundTrip == state));
            }
            stateIds.add(stateId);
        }
        return List.copyOf(stateIds);
    }

    static Map<Identifier, byte[]> generatedResources(
            LoaderActivatedContent content) {
        if (content.blocks().isEmpty()) {
            return Map.of();
        }
        Map<Identifier, byte[]> resources = new LinkedHashMap<>();
        List<String> blockIds = content.blocks().keySet().stream().sorted().toList();
        for (int index = 0; index < blockIds.size(); index++) {
            LoaderBlockDefinition definition = content.blocks().get(blockIds.get(index));
            String path = LoaderBlockCarrier.path(index);
            resources.put(
                    Identifier.fromNamespaceAndPath(
                            "solaris_loader", "blockstates/" + path + ".json"),
                    """
                    {"variants":{"":{"model":"%s"}}}
                    """.formatted(definition.model()).getBytes(StandardCharsets.UTF_8));
            resources.put(
                    Identifier.fromNamespaceAndPath(
                            "solaris_loader", "items/" + path + ".json"),
                    """
                    {"model":{"type":"minecraft:model","model":"%s"}}
                    """.formatted(definition.model()).getBytes(StandardCharsets.UTF_8));
        }
        return Map.copyOf(resources);
    }

    private static int carrierIndex(LoaderActivatedContent content, String blockId) {
        List<String> blockIds = content.blocks().keySet().stream().sorted().toList();
        int index = blockIds.indexOf(blockId);
        if (index < 0 || index >= LoaderBlockCarrier.MAX_CARRIERS) {
            throw new IllegalStateException("Solaris Loader block has no registered carrier");
        }
        return index;
    }

    private static Optional<ItemStack> create(
            LoaderBlockDefinition definition,
            int carrierIndex) {
        Identifier carrierId = LoaderBlockCarrier.id(carrierIndex);
        if (!BuiltInRegistries.ITEM.containsKey(carrierId)) {
            return Optional.empty();
        }
        ItemStack stack =
                new ItemStack(BuiltInRegistries.ITEM.getValue(carrierId));
        stack.set(
                DataComponents.CUSTOM_NAME,
                Component.literal(definition.name()));
        return Optional.of(stack);
    }
}
