package dev.solaris.loader.fabric;

import dev.solaris.loader.minecraft.LoaderBlockCarrier;
import net.fabricmc.api.ModInitializer;
import net.minecraft.core.Registry;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.block.Block;

public final class SolarisFabricContent implements ModInitializer {
    @Override
    public void onInitialize() {
        for (int index = 0; index < LoaderBlockCarrier.MAX_CARRIERS; index++) {
            Block block = Registry.register(
                    BuiltInRegistries.BLOCK,
                    LoaderBlockCarrier.id(index),
                    LoaderBlockCarrier.createBlock(index));
            Registry.register(
                    BuiltInRegistries.ITEM,
                    LoaderBlockCarrier.id(index),
                    LoaderBlockCarrier.createItem(index, block));
        }
    }
}
