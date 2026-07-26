package dev.solaris.loader.minecraft;

import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.item.Item;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockBehaviour;

public final class LoaderBlockCarrier {
    public static final int MAX_CARRIERS = 8;

    private LoaderBlockCarrier() {
    }

    public static String path(int index) {
        requireIndex(index);
        return index == 0 ? "loader_block" : "loader_block_" + index;
    }

    public static Identifier id(int index) {
        return Identifier.fromNamespaceAndPath("solaris_loader", path(index));
    }

    public static Block createBlock(int index) {
        ResourceKey<Block> key = ResourceKey.create(Registries.BLOCK, id(index));
        return new Block(BlockBehaviour.Properties.of()
                .strength(1.5F, 6.0F)
                .setId(key));
    }

    public static BlockItem createItem(int index, Block block) {
        ResourceKey<Item> key = ResourceKey.create(Registries.ITEM, id(index));
        return new BlockItem(
                block,
                new Item.Properties()
                        .setId(key)
                        .useBlockDescriptionPrefix());
    }

    private static void requireIndex(int index) {
        if (index < 0 || index >= MAX_CARRIERS) {
            throw new IllegalArgumentException("Loader carrier index is outside 0..7");
        }
    }
}
