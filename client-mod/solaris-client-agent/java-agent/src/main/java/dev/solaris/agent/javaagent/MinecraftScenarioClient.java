package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientTaskExecutor;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.screens.DeathScreen;
import net.minecraft.client.gui.screens.inventory.SignEditScreen;
import net.minecraft.commands.arguments.EntityAnchorArgument;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.network.protocol.game.ServerboundClientCommandPacket;
import net.minecraft.network.protocol.game.ServerboundPlayerActionPacket;
import net.minecraft.network.protocol.game.ServerboundPlaceRecipePacket;
import net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket;
import net.minecraft.network.protocol.game.ServerboundSignUpdatePacket;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.InteractionResult;
import net.minecraft.world.inventory.AbstractContainerMenu;
import net.minecraft.world.inventory.ContainerInput;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.crafting.display.RecipeDisplayId;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.entity.SignBlockEntity;
import net.minecraft.world.level.block.entity.SignText;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.Vec3;

import java.time.Duration;
import java.util.List;
import java.util.Locale;
import java.util.Objects;

final class MinecraftScenarioClient implements ScenarioClient {
    private static final double SURVIVAL_REACH_SQUARED = 20.25;
    private static final double FAR_REACH_SQUARED = 25.0;
    private static final int FAR_SCAN_RADIUS = 12;
    private static final Duration DIRT_LIKE_SERVER_BREAK_WINDOW = Duration.ofMillis(900);
    private static final Direction[] HORIZONTAL_DIRECTIONS = {
        Direction.EAST,
        Direction.WEST,
        Direction.SOUTH,
        Direction.NORTH
    };
    private static final Direction[] PLACE_DIRECTIONS = {
        Direction.UP,
        Direction.EAST,
        Direction.WEST,
        Direction.SOUTH,
        Direction.NORTH
    };

    private final ClientTaskExecutor executor;

    MinecraftScenarioClient(ClientTaskExecutor executor) {
        this.executor = executor;
    }

    @Override
    public ScenarioBlockPair findOccupiedPair(ScenarioReach reach) throws Exception {
        return executor.callOnClientThread(() -> findOccupiedPairOnClientThread(reach));
    }

    @Override
    public ScenarioBlockPair findPlaceablePair(ScenarioReach reach) throws Exception {
        return executor.callOnClientThread(() -> findPlaceablePairOnClientThread(reach, false));
    }

    @Override
    public ScenarioBlockPair findDryPlaceablePair(ScenarioReach reach) throws Exception {
        return executor.callOnClientThread(() -> findPlaceablePairOnClientThread(reach, true));
    }

    @Override
    public ScenarioHeldItem giveAndSelect(String itemId, int count, int hotbarSlot, Duration timeout)
        throws Exception {
        executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            minecraft.getConnection().sendCommand("debug give " + itemId + " " + count + " " + hotbarSlot);
            selectHotbarSlotOnClientThread(hotbarSlot);
            return null;
        });

        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        ScenarioHeldItem latest;
        do {
            latest = executor.callOnClientThread(() -> {
                selectHotbarSlotOnClientThread(hotbarSlot);
                return selectedItemOnClientThread();
            });
            if (latest.matches(itemId, count)) {
                return latest;
            }
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return latest;
    }

    @Override
    public ScenarioUseResult useItemOn(ScenarioBlockTarget clicked, ScenarioHeldItem heldItem) throws Exception {
        return executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            BlockHitResult hit = hitResult(clicked);
            minecraft.hitResult = hit;
            minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, hit.getLocation());
            InteractionResult result = minecraft.gameMode.useItemOn(
                minecraft.player,
                InteractionHand.MAIN_HAND,
                hit
            );
            minecraft.player.swing(InteractionHand.MAIN_HAND);
            return new ScenarioUseResult(result.toString());
        });
    }

    @Override
    public boolean waitForBlock(ScenarioBlockTarget target, String blockId, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            finalSample = executor.callOnClientThread(() -> Objects.equals(blockIdAt(target), blockId));
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return finalSample;
    }

    @Override
    public boolean waitForAnyBlock(ScenarioBlockTarget target, List<String> blockIds, Duration duration)
        throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            finalSample = executor.callOnClientThread(() -> blockIds.contains(blockIdAt(target)));
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return finalSample;
    }

    @Override
    public boolean waitForStableBlocks(ScenarioBlockPair pair, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            finalSample = executor.callOnClientThread(() ->
                Objects.equals(blockIdAt(pair.clicked()), pair.clicked().blockId())
                    && Objects.equals(blockIdAt(pair.target()), pair.target().blockId())
            );
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return finalSample;
    }

    @Override
    public boolean waitForNoFluid(ScenarioBlockTarget target, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            finalSample = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                return minecraft.level.getFluidState(pos(target)).isEmpty();
            });
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return finalSample;
    }

    @Override
    public boolean waitForSignEditor(ScenarioBlockTarget target, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            finalSample = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                return minecraft.screen instanceof SignEditScreen
                    && minecraft.level.getBlockEntity(pos(target)) instanceof SignBlockEntity;
            });
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return finalSample;
    }

    @Override
    public void updateSignText(ScenarioBlockTarget target, List<String> lines) throws Exception {
        requireFourSignLines(lines);
        executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            minecraft.getConnection().send(new ServerboundSignUpdatePacket(
                pos(target),
                true,
                lines.get(0),
                lines.get(1),
                lines.get(2),
                lines.get(3)
            ));
            return null;
        });
    }

    @Override
    public boolean waitForSignText(ScenarioBlockTarget target, List<String> lines, Duration duration)
        throws Exception {
        requireFourSignLines(lines);
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            finalSample = executor.callOnClientThread(() -> signTextMatches(target, lines));
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return finalSample;
    }

    @Override
    public void placeRecipe(int containerId, int recipeDisplayId, boolean useMaxItems) throws Exception {
        executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            minecraft.getConnection().send(new ServerboundPlaceRecipePacket(
                containerId,
                new RecipeDisplayId(recipeDisplayId),
                useMaxItems
            ));
            return null;
        });
    }

    @Override
    public int inventoryCount(String itemId) throws Exception {
        return executor.callOnClientThread(() -> inventoryCountOnClientThread(itemId));
    }

    @Override
    public boolean waitForInventoryCount(String itemId, int count, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            finalSample = executor.callOnClientThread(() -> inventoryCountOnClientThread(itemId) == count);
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return finalSample;
    }

    @Override
    public boolean waitForScreenClassName(String className, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            finalSample = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                return minecraft.screen != null
                    && Objects.equals(minecraft.screen.getClass().getName(), className);
            });
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return finalSample;
    }

    @Override
    public boolean closeCurrentScreen(Duration duration) throws Exception {
        executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            if (minecraft.screen != null) {
                minecraft.player.closeContainer();
                minecraft.setScreen(null);
            }
            return null;
        });

        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            finalSample = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                return minecraft.screen == null;
            });
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return finalSample;
    }

    @Override
    public boolean moveSelectedItemToContainerSlot(
        int containerSlot,
        String itemId,
        int count,
        Duration duration
    ) throws Exception {
        executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            AbstractContainerMenu menu = minecraft.player.containerMenu;
            int sourceSlot = findMenuSlotWithItem(menu, itemId, count, containerSlot);
            if (sourceSlot < 0) {
                return null;
            }
            minecraft.gameMode.handleContainerInput(
                menu.containerId,
                sourceSlot,
                0,
                ContainerInput.QUICK_MOVE,
                minecraft.player
            );
            return null;
        });
        return waitForContainerSlot(containerSlot, itemId, count, duration);
    }

    @Override
    public boolean waitForContainerSlot(
        int containerSlot,
        String itemId,
        int count,
        Duration duration
    ) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            finalSample = executor.callOnClientThread(
                () -> containerSlotMatchesOnClientThread(containerSlot, itemId, count)
            );
            if (finalSample) {
                return true;
            }
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return finalSample;
    }

    @Override
    public boolean moveContainerSlotToInventory(
        int containerSlot,
        String itemId,
        int count,
        Duration duration
    ) throws Exception {
        executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            AbstractContainerMenu menu = minecraft.player.containerMenu;
            if (
                containerSlot < 0
                    || containerSlot >= menu.slots.size()
                    || !itemStackMatches(menu.getSlot(containerSlot).getItem(), itemId, count)
            ) {
                return null;
            }
            minecraft.gameMode.handleContainerInput(
                menu.containerId,
                containerSlot,
                0,
                ContainerInput.QUICK_MOVE,
                minecraft.player
            );
            return null;
        });
        return waitForContainerSlotEmpty(containerSlot, duration);
    }

    @Override
    public boolean waitForContainerSlotEmpty(int containerSlot, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            finalSample = executor.callOnClientThread(
                () -> containerSlotEmptyOnClientThread(containerSlot)
            );
            if (finalSample) {
                return true;
            }
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return finalSample;
    }

    @Override
    public ScenarioEntityObservation summonEntityNearPlayer(
        String entityTypeId,
        double offsetX,
        double offsetY,
        double offsetZ,
        Duration timeout
    ) throws Exception {
        Vec3 target = executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            Vec3 summonTarget = new Vec3(
                minecraft.player.getX() + offsetX,
                minecraft.player.getY() + offsetY,
                minecraft.player.getZ() + offsetZ
            );
            String command = String.format(
                Locale.ROOT,
                "summon %s %.3f %.3f %.3f",
                entityTypeId,
                summonTarget.x,
                summonTarget.y,
                summonTarget.z
            );
            minecraft.getConnection().sendCommand(command);
            return summonTarget;
        });

        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        ScenarioEntityObservation finalSample = null;
        do {
            finalSample = executor.callOnClientThread(() -> visibleEntityNearOnClientThread(
                entityTypeId,
                target,
                64.0
            ));
            if (finalSample != null) {
                return finalSample;
            }
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return finalSample;
    }

    @Override
    public void sendCommand(String command) throws Exception {
        executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            minecraft.getConnection().sendCommand(command);
            return null;
        });
    }

    @Override
    public boolean teleportTo(double x, double y, double z, Duration timeout) throws Exception {
        executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            minecraft.getConnection().sendCommand(String.format(
                Locale.ROOT,
                "tp %.3f %.3f %.3f",
                x,
                y,
                z
            ));
            return null;
        });

        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        boolean finalSample = false;
        do {
            finalSample = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                double dx = minecraft.player.getX() - x;
                double dy = minecraft.player.getY() - y;
                double dz = minecraft.player.getZ() - z;
                return dx * dx + dy * dy + dz * dz <= 0.75 * 0.75;
            });
            if (finalSample) {
                return true;
            }
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return finalSample;
    }

    @Override
    public boolean waitForDeathScreen(Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            finalSample = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                return minecraft.screen instanceof DeathScreen;
            });
            if (finalSample) {
                return true;
            }
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return false;
    }

    @Override
    public boolean performRespawn(Duration duration) throws Exception {
        executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            minecraft.getConnection().send(new ServerboundClientCommandPacket(
                ServerboundClientCommandPacket.Action.PERFORM_RESPAWN
            ));
            return null;
        });

        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            finalSample = executor.callOnClientThread(() -> {
                Minecraft minecraft = Minecraft.getInstance();
                return minecraft.player != null
                    && minecraft.level != null
                    && minecraft.screen == null
                    && minecraft.player.getHealth() > 0.0F;
            });
            if (finalSample) {
                return true;
            }
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return false;
    }

    @Override
    public ScenarioBreakResult breakBlock(
        ScenarioBlockTarget target,
        String expectedDropItemId,
        int expectedSelectedCount,
        Duration timeout
    ) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        Direction face = direction(target.face());
        boolean started = executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            BlockHitResult hit = hitResult(target);
            minecraft.hitResult = hit;
            minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, hit.getLocation());
            minecraft.getConnection().send(new ServerboundPlayerActionPacket(
                ServerboundPlayerActionPacket.Action.START_DESTROY_BLOCK,
                pos(target),
                face
            ));
            minecraft.player.swing(InteractionHand.MAIN_HAND);
            return true;
        });

        boolean sawDrop = false;
        boolean becameAir = false;
        boolean stopSent = false;
        long stopAfterNanos = System.nanoTime() + DIRT_LIKE_SERVER_BREAK_WINDOW.toNanos();
        ScenarioHeldItem selected = selectedItem();
        try {
            do {
                BreakSample sample = executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    BlockHitResult hit = hitResult(target);
                    minecraft.hitResult = hit;
                    minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, hit.getLocation());
                    minecraft.player.swing(InteractionHand.MAIN_HAND);
                    return new BreakSample(
                        minecraft.level.getBlockState(pos(target)).isAir(),
                        itemDropVisibleOnClientThread(expectedDropItemId, pos(target)),
                        selectedItemOnClientThread()
                    );
                });
                sawDrop |= sample.sawDrop();
                becameAir |= sample.becameAir();
                selected = sample.selectedItem();
                if (!stopSent && System.nanoTime() >= stopAfterNanos) {
                    executor.callOnClientThread(() -> {
                        Minecraft minecraft = requireInPlay();
                        minecraft.getConnection().send(new ServerboundPlayerActionPacket(
                            ServerboundPlayerActionPacket.Action.STOP_DESTROY_BLOCK,
                            pos(target),
                            face
                        ));
                        minecraft.player.swing(InteractionHand.MAIN_HAND);
                        return null;
                    });
                    stopSent = true;
                }
                if (becameAir && selected.matches(expectedDropItemId, expectedSelectedCount)) {
                    return new ScenarioBreakResult(started, true, sawDrop, true, selected);
                }
                Thread.sleep(50);
            } while (System.nanoTime() < deadlineNanos);
            return new ScenarioBreakResult(
                started,
                becameAir,
                sawDrop,
                selected.matches(expectedDropItemId, expectedSelectedCount),
                selected
            );
        } finally {
            if (!stopSent) {
                executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    minecraft.getConnection().send(new ServerboundPlayerActionPacket(
                        ServerboundPlayerActionPacket.Action.ABORT_DESTROY_BLOCK,
                        pos(target),
                        face
                    ));
                    return null;
                });
            }
        }
    }

    @Override
    public ScenarioBreakResult breakBlockUntilDropVisible(
        ScenarioBlockTarget target,
        String expectedDropItemId,
        Duration timeout
    ) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        Direction face = direction(target.face());
        boolean started = executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            BlockHitResult hit = hitResult(target);
            minecraft.hitResult = hit;
            minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, hit.getLocation());
            minecraft.getConnection().send(new ServerboundPlayerActionPacket(
                ServerboundPlayerActionPacket.Action.START_DESTROY_BLOCK,
                pos(target),
                face
            ));
            minecraft.player.swing(InteractionHand.MAIN_HAND);
            return true;
        });

        boolean sawDrop = false;
        boolean becameAir = false;
        boolean stopSent = false;
        long stopAfterNanos = System.nanoTime() + DIRT_LIKE_SERVER_BREAK_WINDOW.toNanos();
        ScenarioHeldItem selected = selectedItem();
        try {
            do {
                BreakSample sample = executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    BlockHitResult hit = hitResult(target);
                    minecraft.hitResult = hit;
                    minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, hit.getLocation());
                    minecraft.player.swing(InteractionHand.MAIN_HAND);
                    return new BreakSample(
                        minecraft.level.getBlockState(pos(target)).isAir(),
                        itemDropVisibleOnClientThread(expectedDropItemId, pos(target)),
                        selectedItemOnClientThread()
                    );
                });
                sawDrop |= sample.sawDrop();
                becameAir |= sample.becameAir();
                selected = sample.selectedItem();
                if (!stopSent && System.nanoTime() >= stopAfterNanos) {
                    executor.callOnClientThread(() -> {
                        Minecraft minecraft = requireInPlay();
                        minecraft.getConnection().send(new ServerboundPlayerActionPacket(
                            ServerboundPlayerActionPacket.Action.STOP_DESTROY_BLOCK,
                            pos(target),
                            face
                        ));
                        minecraft.player.swing(InteractionHand.MAIN_HAND);
                        return null;
                    });
                    stopSent = true;
                }
                if (becameAir && sawDrop) {
                    return new ScenarioBreakResult(started, true, true, false, selected);
                }
                Thread.sleep(50);
            } while (System.nanoTime() < deadlineNanos);
            return new ScenarioBreakResult(started, becameAir, sawDrop, false, selected);
        } finally {
            if (!stopSent) {
                executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    minecraft.getConnection().send(new ServerboundPlayerActionPacket(
                        ServerboundPlayerActionPacket.Action.ABORT_DESTROY_BLOCK,
                        pos(target),
                        face
                    ));
                    return null;
                });
            }
        }
    }

    @Override
    public boolean waitForVisibleItemDrop(String itemId, ScenarioBlockTarget near, Duration timeout)
        throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        boolean finalSample = false;
        do {
            finalSample = executor.callOnClientThread(() -> itemDropVisibleOnClientThread(itemId, pos(near)));
            if (finalSample) {
                return true;
            }
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return false;
    }

    @Override
    public ScenarioBreakResult collectVisibleItemDrop(
        ScenarioBlockTarget near,
        String expectedDropItemId,
        int expectedSelectedCount,
        Duration timeout
    ) throws Exception {
        int initialCount = executor.callOnClientThread(() -> inventoryCountOnClientThread(expectedDropItemId));
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        boolean sawDrop = false;
        boolean dropGone = false;
        boolean pickupRestored = false;
        ScenarioHeldItem selected = selectedItem();
        try {
            do {
                PickupSample sample = executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    Vec3 center = Vec3.atLowerCornerWithOffset(pos(near), 0.5, 0.5, 0.5);
                    minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, center);
                    minecraft.options.keySprint.setDown(true);
                    minecraft.options.keyUp.setDown(true);
                    boolean visible = itemDropVisibleOnClientThread(expectedDropItemId, pos(near));
                    return new PickupSample(
                        visible,
                        selectedItemOnClientThread(),
                        inventoryCountOnClientThread(expectedDropItemId)
                    );
                });
                sawDrop |= sample.visibleDrop();
                dropGone = !sample.visibleDrop();
                selected = sample.selectedItem();
                pickupRestored = selected.matches(expectedDropItemId, expectedSelectedCount)
                    || sample.inventoryCount() >= initialCount + expectedSelectedCount;
                if (dropGone && pickupRestored) {
                    return new ScenarioBreakResult(true, true, sawDrop, true, selected);
                }
                Thread.sleep(50);
            } while (System.nanoTime() < deadlineNanos);
            return new ScenarioBreakResult(true, dropGone, sawDrop, pickupRestored, selected);
        } finally {
            executor.callOnClientThread(() -> {
                Minecraft minecraft = Minecraft.getInstance();
                if (minecraft.options != null) {
                    minecraft.options.keyUp.setDown(false);
                    minecraft.options.keySprint.setDown(false);
                }
                return null;
            });
        }
    }

    @Override
    public boolean waitForNoVisibleItemDrop(String itemId, ScenarioBlockTarget near, Duration timeout)
        throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        boolean finalSample = true;
        do {
            finalSample = executor.callOnClientThread(() -> itemDropVisibleOnClientThread(itemId, pos(near)));
            if (!finalSample) {
                return true;
            }
            Thread.sleep(50);
        } while (System.nanoTime() < deadlineNanos);
        return !finalSample;
    }

    @Override
    public ScenarioHeldItem selectedItem() throws Exception {
        return executor.callOnClientThread(MinecraftScenarioClient::selectedItemOnClientThread);
    }

    static void selectHotbarSlotOnClientThread(int slot) {
        Minecraft minecraft = requireInPlay();
        if (slot < 0 || slot > 8) {
            throw new IllegalArgumentException("hotbar slot must be 0..8");
        }
        minecraft.player.getInventory().setSelectedSlot(slot);
        minecraft.getConnection().send(new ServerboundSetCarriedItemPacket(slot));
    }

    static void lookAtBlockOnClientThread(ScenarioBlockTarget target) {
        Minecraft minecraft = requireInPlay();
        minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, hitResult(target).getLocation());
    }

    static ScenarioUseResult useItemOnClientThread(ScenarioBlockTarget target) {
        Minecraft minecraft = requireInPlay();
        BlockHitResult hit = hitResult(target);
        minecraft.hitResult = hit;
        minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, hit.getLocation());
        InteractionResult result = minecraft.gameMode.useItemOn(
            minecraft.player,
            InteractionHand.MAIN_HAND,
            hit
        );
        minecraft.player.swing(InteractionHand.MAIN_HAND);
        return new ScenarioUseResult(result.toString());
    }

    private static ScenarioBlockPair findOccupiedPairOnClientThread(ScenarioReach reach) {
        Minecraft minecraft = requireInPlay();
        BlockPos origin = minecraft.player.blockPosition();
        Vec3 eye = minecraft.player.getEyePosition();
        for (int radius = 1; radius <= FAR_SCAN_RADIUS; radius++) {
            for (int dx = -radius; dx <= radius; dx++) {
                for (int dz = -radius; dz <= radius; dz++) {
                    if (Math.max(Math.abs(dx), Math.abs(dz)) != radius) {
                        continue;
                    }
                    for (int dy = -3; dy <= 1; dy++) {
                        BlockPos clicked = origin.offset(dx, dy, dz);
                        ScenarioBlockPair pair = firstSolidNeighbourPair(clicked, eye, reach);
                        if (pair != null) {
                            return pair;
                        }
                    }
                }
            }
        }
        return null;
    }

    private static ScenarioBlockPair findPlaceablePairOnClientThread(ScenarioReach reach, boolean requireDryTarget) {
        Minecraft minecraft = requireInPlay();
        BlockPos origin = minecraft.player.blockPosition();
        Vec3 eye = minecraft.player.getEyePosition();
        for (int radius = 1; radius <= FAR_SCAN_RADIUS; radius++) {
            for (int dx = -radius; dx <= radius; dx++) {
                for (int dz = -radius; dz <= radius; dz++) {
                    if (Math.max(Math.abs(dx), Math.abs(dz)) != radius) {
                        continue;
                    }
                    for (int dy = -3; dy <= 1; dy++) {
                        BlockPos clicked = origin.offset(dx, dy, dz);
                        ScenarioBlockPair pair = firstPlaceableNeighbourPair(
                            clicked,
                            eye,
                            reach,
                            requireDryTarget
                        );
                        if (pair != null) {
                            return pair;
                        }
                    }
                }
            }
        }
        return null;
    }

    private static ScenarioBlockPair firstSolidNeighbourPair(
        BlockPos clicked,
        Vec3 eye,
        ScenarioReach reach
    ) {
        if (!isSolidLoaded(clicked)) {
            return null;
        }
        for (Direction direction : HORIZONTAL_DIRECTIONS) {
            BlockPos target = clicked.relative(direction);
            if (!isSolidLoaded(target)) {
                continue;
            }
            double distance = eye.distanceToSqr(Vec3.atLowerCornerWithOffset(
                clicked,
                cursorX(direction),
                0.5,
                cursorZ(direction)
            ));
            if (reach == ScenarioReach.WITHIN_SURVIVAL_REACH && distance > SURVIVAL_REACH_SQUARED) {
                continue;
            }
            if (reach == ScenarioReach.OUTSIDE_SURVIVAL_REACH && distance <= FAR_REACH_SQUARED) {
                continue;
            }
            String face = direction.getName();
            return new ScenarioBlockPair(
                new ScenarioBlockTarget(
                    clicked.getX(),
                    clicked.getY(),
                    clicked.getZ(),
                    face,
                    reach.label() + "-clicked",
                    blockIdAt(clicked)
                ),
                new ScenarioBlockTarget(
                    target.getX(),
                    target.getY(),
                    target.getZ(),
                    direction.getOpposite().getName(),
                    reach.label() + "-target",
                    blockIdAt(target)
                )
            );
        }
        return null;
    }

    private static ScenarioBlockPair firstPlaceableNeighbourPair(
        BlockPos clicked,
        Vec3 eye,
        ScenarioReach reach,
        boolean requireDryTarget
    ) {
        if (!isSolidLoaded(clicked)) {
            return null;
        }
        String clickedBlockId = blockIdAt(clicked);
        if (!dropsAsDirt(clickedBlockId)) {
            return null;
        }
        for (Direction direction : PLACE_DIRECTIONS) {
            BlockPos target = clicked.relative(direction);
            if (!isEmptyLoaded(target) || isPlayerSpace(target)) {
                continue;
            }
            if (requireDryTarget && !isFluidNeighbourhoodEmpty(target)) {
                continue;
            }
            double distance = eye.distanceToSqr(Vec3.atLowerCornerWithOffset(
                clicked,
                cursorX(direction),
                cursorY(direction),
                cursorZ(direction)
            ));
            if (reach == ScenarioReach.WITHIN_SURVIVAL_REACH && distance > SURVIVAL_REACH_SQUARED) {
                continue;
            }
            if (reach == ScenarioReach.OUTSIDE_SURVIVAL_REACH && distance <= FAR_REACH_SQUARED) {
                continue;
            }
            return new ScenarioBlockPair(
                new ScenarioBlockTarget(
                    clicked.getX(),
                    clicked.getY(),
                    clicked.getZ(),
                    direction.getName(),
                    reach.label() + "-place-clicked",
                    clickedBlockId
                ),
                new ScenarioBlockTarget(
                    target.getX(),
                    target.getY(),
                    target.getZ(),
                    direction.getOpposite().getName(),
                    reach.label() + "-place-target",
                    blockIdAt(target)
                )
            );
        }
        return null;
    }

    private static boolean dropsAsDirt(String blockId) {
        return switch (blockId) {
            case "minecraft:dirt", "minecraft:coarse_dirt", "minecraft:grass_block", "minecraft:podzol" -> true;
            default -> false;
        };
    }

    private static boolean isSolidLoaded(BlockPos pos) {
        Minecraft minecraft = requireInPlay();
        if (!minecraft.level.isLoaded(pos)) {
            return false;
        }
        BlockState state = minecraft.level.getBlockState(pos);
        return !state.isAir() && state.blocksMotion();
    }

    private static boolean isEmptyLoaded(BlockPos pos) {
        Minecraft minecraft = requireInPlay();
        if (!minecraft.level.isLoaded(pos)) {
            return false;
        }
        return minecraft.level.getBlockState(pos).isAir()
            && minecraft.level.getFluidState(pos).isEmpty();
    }

    private static boolean isFluidNeighbourhoodEmpty(BlockPos pos) {
        Minecraft minecraft = requireInPlay();
        for (Direction direction : Direction.values()) {
            BlockPos neighbour = pos.relative(direction);
            if (!minecraft.level.isLoaded(neighbour)) {
                return false;
            }
            if (!minecraft.level.getFluidState(neighbour).isEmpty()) {
                return false;
            }
        }
        return true;
    }

    private static boolean isPlayerSpace(BlockPos pos) {
        Minecraft minecraft = requireInPlay();
        BlockPos feet = minecraft.player.blockPosition();
        return pos.equals(feet) || pos.equals(feet.above());
    }

    private static boolean itemDropVisibleOnClientThread(String itemId, BlockPos near) {
        Minecraft minecraft = requireInPlay();
        Vec3 center = Vec3.atLowerCornerWithOffset(near, 0.5, 0.5, 0.5);
        for (Entity entity : minecraft.level.entitiesForRendering()) {
            if (entity instanceof ItemEntity itemEntity && !entity.isRemoved()) {
                ItemStack stack = itemEntity.getItem();
                if (
                    !stack.isEmpty()
                        && Objects.equals(BuiltInRegistries.ITEM.getKey(stack.getItem()).toString(), itemId)
                        && entity.distanceToSqr(center) <= 16.0
                ) {
                    return true;
                }
            }
        }
        return false;
    }

    private static ScenarioEntityObservation visibleEntityNearOnClientThread(
        String entityTypeId,
        Vec3 target,
        double maxDistanceSquared
    ) {
        Minecraft minecraft = requireInPlay();
        Vec3 playerPosition = minecraft.player.position();
        for (Entity entity : minecraft.level.entitiesForRendering()) {
            if (entity.isRemoved()) {
                continue;
            }
            String observedTypeId = BuiltInRegistries.ENTITY_TYPE.getKey(entity.getType()).toString();
            if (
                Objects.equals(observedTypeId, entityTypeId)
                    && entity.distanceToSqr(target) <= maxDistanceSquared
            ) {
                return new ScenarioEntityObservation(
                    observedTypeId,
                    entity.getId(),
                    entity.getX(),
                    entity.getY(),
                    entity.getZ(),
                    entity.distanceToSqr(playerPosition)
                );
            }
        }
        return null;
    }

    private static boolean signTextMatches(ScenarioBlockTarget target, List<String> lines) {
        Minecraft minecraft = requireInPlay();
        BlockEntity blockEntity = minecraft.level.getBlockEntity(pos(target));
        if (!(blockEntity instanceof SignBlockEntity sign)) {
            return false;
        }
        SignText frontText = sign.getText(true);
        for (int index = 0; index < lines.size(); index++) {
            if (!Objects.equals(frontText.getMessage(index, false).getString(), lines.get(index))) {
                return false;
            }
        }
        return true;
    }

    private static void requireFourSignLines(List<String> lines) {
        if (lines.size() != 4) {
            throw new IllegalArgumentException("sign text update requires exactly four lines");
        }
    }

    private static ScenarioHeldItem selectedItemOnClientThread() {
        Minecraft minecraft = requireInPlay();
        ItemStack stack = minecraft.player.getInventory().getSelectedItem();
        if (stack.isEmpty()) {
            return new ScenarioHeldItem("minecraft:air", 0);
        }
        return new ScenarioHeldItem(BuiltInRegistries.ITEM.getKey(stack.getItem()).toString(), stack.getCount());
    }

    private static int inventoryCountOnClientThread(String itemId) {
        Minecraft minecraft = requireInPlay();
        int total = 0;
        for (int slot = 0; slot < minecraft.player.getInventory().getContainerSize(); slot++) {
            ItemStack stack = minecraft.player.getInventory().getItem(slot);
            if (
                !stack.isEmpty()
                    && Objects.equals(BuiltInRegistries.ITEM.getKey(stack.getItem()).toString(), itemId)
            ) {
                total += stack.getCount();
            }
        }
        return total;
    }

    private static int findMenuSlotWithItem(
        AbstractContainerMenu menu,
        String itemId,
        int count,
        int excludedSlot
    ) {
        for (int slot = menu.slots.size() - 1; slot >= 0; slot--) {
            if (slot == excludedSlot) {
                continue;
            }
            ItemStack stack = menu.getSlot(slot).getItem();
            if (itemStackMatches(stack, itemId, count)) {
                return slot;
            }
        }
        return -1;
    }

    private static boolean containerSlotMatchesOnClientThread(
        int containerSlot,
        String itemId,
        int count
    ) {
        Minecraft minecraft = requireInPlay();
        AbstractContainerMenu menu = minecraft.player.containerMenu;
        if (containerSlot < 0 || containerSlot >= menu.slots.size()) {
            return false;
        }
        return itemStackMatches(menu.getSlot(containerSlot).getItem(), itemId, count);
    }

    private static boolean containerSlotEmptyOnClientThread(int containerSlot) {
        Minecraft minecraft = requireInPlay();
        AbstractContainerMenu menu = minecraft.player.containerMenu;
        if (containerSlot < 0 || containerSlot >= menu.slots.size()) {
            return false;
        }
        return menu.getSlot(containerSlot).getItem().isEmpty();
    }

    private static boolean itemStackMatches(ItemStack stack, String itemId, int count) {
        return !stack.isEmpty()
            && stack.getCount() >= count
            && Objects.equals(BuiltInRegistries.ITEM.getKey(stack.getItem()).toString(), itemId);
    }

    private static String blockIdAt(ScenarioBlockTarget target) {
        return blockIdAt(pos(target));
    }

    private static String blockIdAt(BlockPos pos) {
        Minecraft minecraft = requireInPlay();
        BlockState state = minecraft.level.getBlockState(pos);
        return BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString();
    }

    private static BlockHitResult hitResult(ScenarioBlockTarget target) {
        Direction direction = direction(target.face());
        return new BlockHitResult(
            Vec3.atLowerCornerWithOffset(pos(target), cursorX(direction), cursorY(direction), cursorZ(direction)),
            direction,
            pos(target),
            false
        );
    }

    private static Direction direction(String face) {
        Direction direction = Direction.byName(face.toLowerCase(Locale.ROOT));
        if (direction == null) {
            throw new IllegalArgumentException("unknown face: " + face);
        }
        return direction;
    }

    private static double cursorX(Direction direction) {
        return switch (direction) {
            case EAST -> 1.0;
            case WEST -> 0.0;
            default -> 0.5;
        };
    }

    private static double cursorY(Direction direction) {
        return switch (direction) {
            case UP -> 1.0;
            case DOWN -> 0.0;
            default -> 0.5;
        };
    }

    private static double cursorZ(Direction direction) {
        return switch (direction) {
            case SOUTH -> 1.0;
            case NORTH -> 0.0;
            default -> 0.5;
        };
    }

    private static BlockPos pos(ScenarioBlockTarget target) {
        return new BlockPos(target.x(), target.y(), target.z());
    }

    private record BreakSample(boolean becameAir, boolean sawDrop, ScenarioHeldItem selectedItem) {
    }

    private record PickupSample(boolean visibleDrop, ScenarioHeldItem selectedItem, int inventoryCount) {
    }

    private static Minecraft requireInPlay() {
        Minecraft minecraft = Minecraft.getInstance();
        if (minecraft.player == null || minecraft.level == null || minecraft.gameMode == null) {
            throw new IllegalStateException("client is not in play");
        }
        if (minecraft.getConnection() == null) {
            throw new IllegalStateException("client connection is not available");
        }
        return minecraft;
    }
}
