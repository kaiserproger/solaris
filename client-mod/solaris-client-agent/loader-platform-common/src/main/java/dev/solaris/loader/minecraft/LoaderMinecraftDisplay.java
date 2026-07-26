package dev.solaris.loader.minecraft;

import dev.solaris.loader.LoaderActivatedContent;
import dev.solaris.loader.LoaderScreenDefinition;
import java.util.ArrayList;
import java.util.List;
import net.minecraft.world.item.ItemStack;

public final class LoaderMinecraftDisplay {
    private LoaderMinecraftDisplay() {
    }

    public static List<ItemStack> forScreen(
            LoaderScreenDefinition screen,
            LoaderActivatedContent content) {
        List<ItemStack> stacks = new ArrayList<>(2);
        LoaderMinecraftItem.forScreen(screen, content).ifPresent(stacks::add);
        LoaderMinecraftBlock.forScreen(screen, content).ifPresent(stacks::add);
        return List.copyOf(stacks);
    }
}
