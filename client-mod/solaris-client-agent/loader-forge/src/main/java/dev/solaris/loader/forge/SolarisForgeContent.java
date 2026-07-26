package dev.solaris.loader.forge;

import dev.solaris.loader.minecraft.LoaderBlockCarrier;
import java.util.List;
import java.util.stream.IntStream;
import net.minecraft.core.registries.Registries;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.item.Item;
import net.minecraft.world.level.block.Block;
import net.minecraftforge.fml.javafmlmod.FMLJavaModLoadingContext;
import net.minecraftforge.registries.DeferredRegister;
import net.minecraftforge.registries.RegistryObject;

final class SolarisForgeContent {
    private static final DeferredRegister<Block> BLOCKS =
            DeferredRegister.create(Registries.BLOCK, SolarisForgeLoader.MOD_ID);
    private static final DeferredRegister<Item> ITEMS =
            DeferredRegister.create(Registries.ITEM, SolarisForgeLoader.MOD_ID);
    private static final List<RegistryObject<Block>> LOADER_BLOCKS =
            IntStream.range(0, LoaderBlockCarrier.MAX_CARRIERS)
                    .mapToObj(index -> BLOCKS.register(
                            LoaderBlockCarrier.path(index),
                            () -> LoaderBlockCarrier.createBlock(index)))
                    .toList();
    @SuppressWarnings("unused")
    private static final List<RegistryObject<BlockItem>> LOADER_BLOCK_ITEMS =
            IntStream.range(0, LoaderBlockCarrier.MAX_CARRIERS)
                    .mapToObj(index -> ITEMS.register(
                            LoaderBlockCarrier.path(index),
                            () -> LoaderBlockCarrier.createItem(
                                    index,
                                    LOADER_BLOCKS.get(index).get())))
                    .toList();

    private SolarisForgeContent() {
    }

    static void register(FMLJavaModLoadingContext context) {
        BLOCKS.register(context.getModBusGroup());
        ITEMS.register(context.getModBusGroup());
    }
}
