package dev.solaris.loader.minecraft;

import dev.solaris.loader.LoaderActivatedContent;
import dev.solaris.loader.LoaderItemDefinition;
import dev.solaris.loader.LoaderScreenDefinition;
import java.util.Optional;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.network.chat.Component;
import net.minecraft.resources.Identifier;
import net.minecraft.world.item.ItemStack;

public final class LoaderMinecraftItem {
    private LoaderMinecraftItem() {
    }

    public static Optional<ItemStack> forScreen(
            LoaderScreenDefinition screen,
            LoaderActivatedContent content) {
        return resolve(screen, content)
                .flatMap(LoaderMinecraftItem::create);
    }

    static Optional<LoaderItemDefinition> resolve(
            LoaderScreenDefinition screen,
            LoaderActivatedContent content) {
        return screen.itemId()
                .map(content.items()::get)
                .filter(java.util.Objects::nonNull);
    }

    static void validate(LoaderActivatedContent content) {
        for (LoaderItemDefinition definition : content.items().values()) {
            Identifier baseItem = Identifier.tryParse(definition.baseItem());
            if (baseItem == null || !BuiltInRegistries.ITEM.containsKey(baseItem)) {
                throw new IllegalArgumentException(
                        "Loader item has unknown base item " + definition.baseItem());
            }
        }
    }

    static Optional<ItemStack> create(LoaderItemDefinition definition) {
        Identifier baseItem = Identifier.tryParse(definition.baseItem());
        Identifier model = Identifier.tryParse(definition.id());
        if (baseItem == null
                || model == null
                || !BuiltInRegistries.ITEM.containsKey(baseItem)) {
            return Optional.empty();
        }
        ItemStack stack = new ItemStack(BuiltInRegistries.ITEM.getValue(baseItem));
        stack.set(DataComponents.ITEM_MODEL, model);
        stack.set(DataComponents.CUSTOM_NAME, Component.literal(definition.name()));
        return Optional.of(stack);
    }
}
