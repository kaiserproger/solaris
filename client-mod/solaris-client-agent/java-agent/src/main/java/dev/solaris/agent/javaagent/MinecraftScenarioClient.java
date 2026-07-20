package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientTaskExecutor;
import net.minecraft.client.Minecraft;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.client.gui.components.ChatComponent;
import net.minecraft.client.gui.screens.DeathScreen;
import net.minecraft.client.gui.screens.inventory.SignEditScreen;
import net.minecraft.client.multiplayer.chat.GuiMessage;
import net.minecraft.commands.arguments.EntityAnchorArgument;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.network.protocol.game.ServerboundClientCommandPacket;
import net.minecraft.network.protocol.game.ServerboundPlaceRecipePacket;
import net.minecraft.network.protocol.game.ServerboundSetCarriedItemPacket;
import net.minecraft.network.protocol.game.ServerboundSignUpdatePacket;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.animal.sheep.Sheep;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.InteractionResult;
import net.minecraft.world.inventory.AbstractContainerMenu;
import net.minecraft.world.inventory.ContainerInput;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.DyeColor;
import net.minecraft.world.item.crafting.display.RecipeDisplayId;
import net.minecraft.world.item.crafting.display.SlotDisplayContext;
import net.minecraft.world.level.LightLayer;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.entity.SignBlockEntity;
import net.minecraft.world.level.block.entity.SignText;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.EntityHitResult;
import net.minecraft.world.phys.shapes.CollisionContext;
import net.minecraft.world.phys.Vec3;

import java.lang.reflect.Field;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.Objects;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicReference;

public final class MinecraftScenarioClient implements ScenarioClient {
    private static final double SURVIVAL_REACH_SQUARED = 20.25;
    private static final double FAR_REACH_SQUARED = 25.0;
    private static final double PICKUP_APPROACH_DISTANCE_SQUARED = 4.0;
    private static final double POSITION_APPROACH_DISTANCE_SQUARED = 2.25;
    private static final int FAR_SCAN_RADIUS = 12;
    private static final int NATURAL_BREAKABLE_SCAN_RADIUS = 64;
    private static final int NATURAL_BREAKABLE_SCAN_DOWN = 32;
    private static final int NATURAL_BREAKABLE_SCAN_UP = 16;
    private static final int APPROACHABLE_SCAN_DOWN = 4;
    private static final int APPROACHABLE_SCAN_UP = 5;
    private static final int[] NATURAL_BREAKABLE_SCAN_VERTICAL_OFFSETS = naturalBreakableScanVerticalOffsets();
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
    private static final Direction[] BREAK_DIRECTIONS = {
        Direction.EAST,
        Direction.WEST,
        Direction.SOUTH,
        Direction.NORTH,
        Direction.UP,
        Direction.DOWN
    };
    private static final AtomicReference<BlockBreakAutomation> ACTIVE_BLOCK_BREAK =
        new AtomicReference<>();

    private final ClientTaskExecutor executor;

    MinecraftScenarioClient(ClientTaskExecutor executor) {
        this.executor = executor;
    }

    public static boolean hasActiveBlockBreak() {
        return ACTIVE_BLOCK_BREAK.get() != null;
    }

    public static void runPreTickActions() {
        BlockBreakAutomation action = ACTIVE_BLOCK_BREAK.get();
        if (action == null) {
            return;
        }

        try {
            Minecraft minecraft = Minecraft.getInstance();
            if (minecraft.player == null || minecraft.level == null || minecraft.gameMode == null) {
                return;
            }
            if (minecraft.screen != null) {
                return;
            }
            BlockHitResult hit = hitResult(action.target);
            minecraft.hitResult = hit;
            minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, hit.getLocation());
            minecraft.options.keyAttack.setDown(true);
            if (!action.startSent) {
                action.startSent = true;
                minecraft.gameMode.startDestroyBlock(pos(action.target), action.face);
                minecraft.player.swing(InteractionHand.MAIN_HAND);
                action.started.complete(null);
            } else if (minecraft.gameMode.continueDestroyBlock(pos(action.target), action.face)) {
                minecraft.player.swing(InteractionHand.MAIN_HAND);
            }
        } catch (RuntimeException error) {
            ACTIVE_BLOCK_BREAK.compareAndSet(action, null);
            action.started.completeExceptionally(error);
        }
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
    public ScenarioBlockPair findTillableSoil(ScenarioReach reach) throws Exception {
        return executor.callOnClientThread(() -> findTillableSoilOnClientThread(reach));
    }

    @Override
    public ScenarioBlockPair findOpenDryPlaceablePair(ScenarioReach reach) throws Exception {
        return executor.callOnClientThread(() -> findPlaceablePairOnClientThread(reach, true, true));
    }

    @Override
    public ScenarioBlockPair findUnobstructedPlaceablePair(ScenarioReach reach) throws Exception {
        return executor.callOnClientThread(() -> findPlaceablePairOnClientThread(reach, false, true));
    }

    @Override
    public ScenarioBlockPair findHorizontalPlaceablePair(ScenarioReach reach) throws Exception {
        return executor.callOnClientThread(() ->
            findPlaceablePairOnClientThread(reach, true, false, HORIZONTAL_DIRECTIONS)
        );
    }

    @Override
    public ScenarioBlockPair findVerticalPlaceablePair(ScenarioReach reach) throws Exception {
        return executor.callOnClientThread(() ->
            findPlaceablePairOnClientThread(reach, true, false, new Direction[] { Direction.UP })
        );
    }

    @Override
    public ScenarioBlockPair findHorizontalAttachmentPair(
        ScenarioBlockTarget support,
        ScenarioReach reach
    ) throws Exception {
        return executor.callOnClientThread(() -> findHorizontalAttachmentPairOnClientThread(support, reach));
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
            long observedVersion = ClientStateEvents.version();
            latest = executor.callOnClientThread(() -> {
                selectHotbarSlotOnClientThread(hotbarSlot);
                return selectedItemOnClientThread();
            });
            if (latest.matches(itemId, count)) {
                return latest;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return latest;
    }

    @Override
    public ScenarioUseResult useItemOn(ScenarioBlockTarget clicked, ScenarioHeldItem heldItem) throws Exception {
        return executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            long ackVersion = ClientStateEvents.blockChangeAckVersion();
            BlockHitResult hit = hitResult(clicked);
            minecraft.hitResult = hit;
            minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, hit.getLocation());
            InteractionResult result = minecraft.gameMode.useItemOn(
                minecraft.player,
                InteractionHand.MAIN_HAND,
                hit
            );
            minecraft.player.swing(InteractionHand.MAIN_HAND);
            return new ScenarioUseResult(result.toString(), ackVersion);
        });
    }

    @Override
    public ScenarioUseResult useItemOnAtHeight(
        ScenarioBlockTarget clicked,
        ScenarioHeldItem heldItem,
        double cursorHeight
    ) throws Exception {
        if (cursorHeight < 0.0 || cursorHeight > 1.0) {
            throw new IllegalArgumentException("cursor height must be between 0 and 1");
        }
        return executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            long ackVersion = ClientStateEvents.blockChangeAckVersion();
            BlockHitResult hit = hitResult(clicked, cursorHeight);
            minecraft.hitResult = hit;
            minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, hit.getLocation());
            InteractionResult result = minecraft.gameMode.useItemOn(
                minecraft.player,
                InteractionHand.MAIN_HAND,
                hit
            );
            minecraft.player.swing(InteractionHand.MAIN_HAND);
            return new ScenarioUseResult(result.toString(), ackVersion);
        });
    }

    @Override
    public boolean waitForUseAcknowledgement(ScenarioUseResult use, Duration timeout)
        throws InterruptedException {
        if (use.blockChangeAckVersionBeforeUse() < 0L) {
            return false;
        }
        return ClientStateEvents.awaitBlockChangeAck(use.blockChangeAckVersionBeforeUse(), timeout);
    }

    @Override
    public boolean waitForBlock(ScenarioBlockTarget target, String blockId, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> Objects.equals(blockIdAt(target), blockId));
            if (finalSample || !awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public boolean waitForAnyBlock(ScenarioBlockTarget target, List<String> blockIds, Duration duration)
        throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> blockIds.contains(blockIdAt(target)));
            if (finalSample || !awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public boolean waitForBlockProperty(
        ScenarioBlockTarget target,
        String property,
        String value,
        Duration duration
    ) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> blockPropertyMatches(target, property, value));
            if (finalSample || !awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public ScenarioLightLevel lightLevel(ScenarioBlockTarget target) throws Exception {
        return executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            BlockPos position = pos(target);
            return new ScenarioLightLevel(
                minecraft.level.getBrightness(LightLayer.SKY, position),
                minecraft.level.getBrightness(LightLayer.BLOCK, position)
            );
        });
    }

    @Override
    public boolean waitForStableBlocks(ScenarioBlockPair pair, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() ->
                Objects.equals(blockIdAt(pair.clicked()), pair.clicked().blockId())
                    && Objects.equals(blockIdAt(pair.target()), pair.target().blockId())
            );
            if (finalSample || !awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public boolean waitForNoFluid(ScenarioBlockTarget target, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                return minecraft.level.getFluidState(pos(target)).isEmpty();
            });
            if (finalSample || !awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public boolean waitForSignEditor(ScenarioBlockTarget target, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                return minecraft.screen instanceof SignEditScreen
                    && minecraft.level.getBlockEntity(pos(target)) instanceof SignBlockEntity;
            });
            if (finalSample || !awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
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
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> signTextMatches(target, lines));
            if (finalSample || !awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
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
    public int recipeDisplayIdForResult(String itemId) throws Exception {
        return executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            var context = SlotDisplayContext.fromLevel(minecraft.level);
            for (var collection : minecraft.player.getRecipeBook().getCollections()) {
                for (var entry : collection.getRecipes()) {
                    boolean matches = entry.resultItems(context).stream().anyMatch(stack ->
                        !stack.isEmpty()
                            && Objects.equals(
                                BuiltInRegistries.ITEM.getKey(stack.getItem()).toString(),
                                itemId
                            )
                    );
                    if (matches) {
                        return entry.id().index();
                    }
                }
            }
            return -1;
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
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> inventoryCountOnClientThread(itemId) == count);
            if (finalSample || !awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public int totalExperience() throws Exception {
        return executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            return minecraft.player.totalExperience;
        });
    }

    @Override
    public int experienceLevel() throws Exception {
        return executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            return minecraft.player.experienceLevel;
        });
    }

    @Override
    public boolean waitForExperience(int totalExperience, int level, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        do {
            long observedVersion = ClientStateEvents.version();
            boolean matched = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                return minecraft.player.totalExperience == totalExperience
                    && minecraft.player.experienceLevel == level;
            });
            if (matched) {
                return true;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                return false;
            }
        } while (true);
    }

    @Override
    public int waitForTotalExperienceAbove(int totalExperience, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        int latest;
        do {
            long observedVersion = ClientStateEvents.version();
            latest = totalExperience();
            if (latest > totalExperience) {
                return latest;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return latest;
    }

    @Override
    public boolean waitForDayTimeAtOrAfter(long dayTime, Duration duration) throws Exception {
        return waitForDayTime(dayTime, duration, true);
    }

    @Override
    public boolean waitForDayTimeBelow(long dayTime, Duration duration) throws Exception {
        return waitForDayTime(dayTime, duration, false);
    }

    private boolean waitForDayTime(long dayTime, Duration duration, boolean atOrAfter) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.tickVersion();
            finalSample = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                long current = Math.floorMod(minecraft.level.getLevelData().getGameTime(), 24_000L);
                return atOrAfter ? current >= dayTime : current < dayTime;
            });
            if (finalSample) {
                return true;
            }
            if (!awaitClientTick(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public boolean waitForScreenClassName(String className, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                return minecraft.screen != null
                    && Objects.equals(minecraft.screen.getClass().getName(), className);
            });
            if (finalSample || !awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public boolean closeCurrentScreen(Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                return closeCurrentScreenOnClientThread(minecraft);
            });
            if (finalSample) {
                return true;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    static boolean closeCurrentScreenOnClientThread(Minecraft minecraft) {
        minecraft.options.pauseOnLostFocus = false;
        if (minecraft.screen != null) {
            minecraft.player.closeContainer();
            minecraft.setScreen(null);
        }
        return minecraft.screen == null;
    }

    @Override
    public int activeContainerId() throws Exception {
        return executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            return minecraft.player.containerMenu.containerId;
        });
    }

    @Override
    public boolean moveSelectedItemToContainerSlot(
        int containerSlot,
        String itemId,
        int count,
        Duration duration
    ) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        do {
            long observedVersion = ClientStateEvents.version();
            ContainerClickAttempt attempt = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                AbstractContainerMenu menu = minecraft.player.containerMenu;
                if (containerSlotMatchesOnClientThread(containerSlot, itemId, count)) {
                    return new ContainerClickAttempt(true, null);
                }
                int sourceSlot = findMenuSlotWithItem(menu, itemId, count, containerSlot);
                if (sourceSlot < 0) {
                    return new ContainerClickAttempt(false, null);
                }
                ContainerUpdateCheckpoint checkpoint = new ContainerUpdateCheckpoint(
                    menu.containerId,
                    menu.getStateId()
                );
                minecraft.gameMode.handleContainerInput(
                    menu.containerId,
                    sourceSlot,
                    0,
                    ContainerInput.QUICK_MOVE,
                    minecraft.player
                );
                return new ContainerClickAttempt(false, checkpoint);
            });
            if (attempt.alreadyMatched()) {
                return true;
            }
            if (attempt.checkpoint() != null) {
                return waitForAuthoritativeContainerUpdate(
                    attempt.checkpoint(),
                    containerSlot,
                    itemId,
                    count,
                    false,
                    deadlineNanos
                );
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return false;
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
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(
                () -> containerSlotMatchesOnClientThread(containerSlot, itemId, count)
            );
            if (finalSample) {
                return true;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public boolean moveContainerSlotToInventory(
        int containerSlot,
        String itemId,
        int count,
        Duration duration
    ) throws Exception {
        ContainerClickAttempt attempt = executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            AbstractContainerMenu menu = minecraft.player.containerMenu;
            if (containerSlotEmptyOnClientThread(containerSlot)) {
                return new ContainerClickAttempt(true, null);
            }
            if (
                containerSlot < 0
                    || containerSlot >= menu.slots.size()
                    || !itemStackMatches(menu.getSlot(containerSlot).getItem(), itemId, count)
            ) {
                return new ContainerClickAttempt(false, null);
            }
            ContainerUpdateCheckpoint checkpoint = new ContainerUpdateCheckpoint(
                menu.containerId,
                menu.getStateId()
            );
            minecraft.gameMode.handleContainerInput(
                menu.containerId,
                containerSlot,
                0,
                ContainerInput.QUICK_MOVE,
                minecraft.player
            );
            return new ContainerClickAttempt(false, checkpoint);
        });
        if (attempt.alreadyMatched()) {
            return true;
        }
        if (attempt.checkpoint() == null) {
            return false;
        }
        return waitForAuthoritativeContainerUpdate(
            attempt.checkpoint(),
            containerSlot,
            itemId,
            count,
            true,
            System.nanoTime() + duration.toNanos()
        );
    }

    private boolean waitForAuthoritativeContainerUpdate(
        ContainerUpdateCheckpoint checkpoint,
        int containerSlot,
        String itemId,
        int count,
        boolean expectEmpty,
        long deadlineNanos
    ) throws Exception {
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                AbstractContainerMenu menu = minecraft.player.containerMenu;
                boolean slotMatches = expectEmpty
                    ? containerSlotEmptyOnClientThread(containerSlot)
                    : containerSlotMatchesOnClientThread(containerSlot, itemId, count);
                return ScenarioClient.authoritativeContainerUpdateMatches(
                    checkpoint.containerId(),
                    checkpoint.stateId(),
                    menu.containerId,
                    menu.getStateId(),
                    slotMatches
                );
            });
            if (finalSample || !awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public boolean waitForContainerSlotEmpty(int containerSlot, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(
                () -> containerSlotEmptyOnClientThread(containerSlot)
            );
            if (finalSample) {
                return true;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public boolean quickMoveContainerSlot(int containerSlot, Duration duration) throws Exception {
        if (containerSlot < 0 || containerSlot > Short.MAX_VALUE) {
            throw new IllegalArgumentException("container slot must be between 0 and " + Short.MAX_VALUE);
        }
        ContainerUpdateCheckpoint checkpoint = executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            AbstractContainerMenu menu = minecraft.player.containerMenu;
            if (containerSlot >= menu.slots.size()) {
                throw new IllegalArgumentException(
                    "container slot " + containerSlot + " is outside menu size " + menu.slots.size()
                );
            }
            ContainerUpdateCheckpoint before = new ContainerUpdateCheckpoint(
                menu.containerId,
                menu.getStateId()
            );
            minecraft.gameMode.handleContainerInput(
                menu.containerId,
                containerSlot,
                0,
                ContainerInput.QUICK_MOVE,
                minecraft.player
            );
            return before;
        });
        return waitForContainerStateAdvance(checkpoint, System.nanoTime() + duration.toNanos());
    }

    @Override
    public int findContainerSlot(String itemId, int count) throws Exception {
        return executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            AbstractContainerMenu menu = minecraft.player.containerMenu;
            int chestSlotLimit = Math.min(27, menu.slots.size());
            for (int slot = 0; slot < chestSlotLimit; slot++) {
                if (itemStackMatches(menu.getSlot(slot).getItem(), itemId, count)) {
                    return slot;
                }
            }
            return -1;
        });
    }

    @Override
    public boolean clickContainerButton(int buttonId, Duration duration) throws Exception {
        if (buttonId < 0) {
            throw new IllegalArgumentException("container button id must be non-negative");
        }
        ContainerUpdateCheckpoint checkpoint = executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            AbstractContainerMenu menu = minecraft.player.containerMenu;
            ContainerUpdateCheckpoint before = new ContainerUpdateCheckpoint(
                menu.containerId,
                menu.getStateId()
            );
            minecraft.gameMode.handleInventoryButtonClick(menu.containerId, buttonId);
            return before;
        });
        return waitForContainerStateAdvance(checkpoint, System.nanoTime() + duration.toNanos());
    }

    @Override
    public boolean containerSlotHasEnchantment(int slot, String enchantmentId, int level) throws Exception {
        return executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            AbstractContainerMenu menu = minecraft.player.containerMenu;
            if (slot < 0 || slot >= menu.slots.size()) {
                return false;
            }
            ItemStack stack = menu.getSlot(slot).getItem();
            return stack.getEnchantments().entrySet().stream().anyMatch(entry ->
                entry.getIntValue() == level
                    && Objects.equals(entry.getKey().getRegisteredName(), enchantmentId)
            );
        });
    }

    private boolean waitForContainerStateAdvance(
        ContainerUpdateCheckpoint checkpoint,
        long deadlineNanos
    ) throws Exception {
        do {
            long observedVersion = ClientStateEvents.version();
            boolean advanced = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                AbstractContainerMenu menu = minecraft.player.containerMenu;
                return menu.containerId == checkpoint.containerId()
                    && menu.getStateId() != checkpoint.stateId();
            });
            if (advanced) {
                return true;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                return false;
            }
        } while (true);
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
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> visibleEntityNearOnClientThread(
                entityTypeId,
                target,
                64.0
            ));
            if (finalSample != null) {
                return finalSample;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
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
    public void sendChatMessage(String message) throws Exception {
        executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            minecraft.getConnection().sendChat(message);
            return null;
        });
    }

    @Override
    public boolean waitForChatMessage(String expectedText, Duration timeout) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> chatHistoryContainsOnClientThread(expectedText));
            if (finalSample) {
                return true;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public boolean waitForTicks(long ticks, Duration timeout) throws Exception {
        if (ticks < 1L) {
            throw new IllegalArgumentException("ticks must be positive");
        }
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        int startTick = executor.callOnClientThread(() -> requireInPlay().player.tickCount);
        while (true) {
            long observedVersion = ClientStateEvents.tickVersion();
            int currentTick = executor.callOnClientThread(() -> requireInPlay().player.tickCount);
            if (Integer.toUnsignedLong(currentTick - startTick) >= ticks) {
                return true;
            }
            if (!awaitClientTick(observedVersion, deadlineNanos)) {
                return false;
            }
        }
    }

    @Override
    public long serverGameTime() {
        return ClientStateEvents.serverGameTime();
    }

    @Override
    public long waitForServerTimeAfter(long baseline, Duration timeout) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        while (true) {
            long observedVersion = ClientStateEvents.serverTimeVersion();
            long gameTime = ClientStateEvents.serverGameTime();
            if (gameTime > baseline) {
                return gameTime;
            }
            long remainingNanos = deadlineNanos - System.nanoTime();
            if (remainingNanos <= 0L || !ClientStateEvents.awaitServerTimeChange(
                observedVersion,
                Duration.ofNanos(remainingNanos)
            )) {
                return gameTime;
            }
        }
    }

    @Override
    public ScenarioEntityObservation findVisibleEntity(
        List<String> entityTypeIds,
        ScenarioReach reach,
        Duration timeout
    ) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        ScenarioEntityObservation finalSample = null;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> visibleEntityOnClientThread(entityTypeIds, reach));
            if (finalSample != null) {
                return finalSample;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public ScenarioEntityObservation findVisibleSheepWithWool(
        String woolItemId,
        ScenarioReach reach,
        Duration timeout
    ) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        ScenarioEntityObservation finalSample = null;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> visibleSheepWithWoolOnClientThread(woolItemId, reach));
            if (finalSample != null) {
                return finalSample;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public ScenarioEntityObservation visibleEntity(List<String> entityTypeIds, ScenarioReach reach)
        throws Exception {
        return executor.callOnClientThread(() -> visibleEntityOnClientThread(entityTypeIds, reach));
    }

    @Override
    public ScenarioEntityInteractionResult interactEntity(ScenarioEntityInteraction interaction)
        throws Exception {
        return EntityInteractionDispatch.queue(
            executor,
            interaction,
            new MinecraftEntityInteractionAccess()
        );
    }

    @Override
    public ScenarioEntityMotionObservation waitForEntityMotion(
        ScenarioEntityObservation entity,
        double minimumHorizontalDistance,
        double minimumVerticalRise,
        Duration timeout
    ) throws Exception {
        if (minimumHorizontalDistance <= 0.0) {
            throw new IllegalArgumentException("minimum horizontal distance must be positive");
        }
        if (minimumVerticalRise < 0.0) {
            throw new IllegalArgumentException("minimum vertical rise must not be negative");
        }
        EntityWaitSource source = entityWaitSource(entity.identity());
        return waitForEntityMotion(
            source,
            entity.entityId(),
            entity.x(),
            entity.y(),
            entity.z(),
            true,
            minimumHorizontalDistance,
            minimumVerticalRise,
            timeout
        );
    }

    public ScenarioEntityMotionObservation waitForEntityMotion(
        int entityId,
        UUID entityUuid,
        String entityTypeId,
        double minimumHorizontalDistance,
        double minimumVerticalRise,
        Duration timeout
    ) throws Exception {
        if (minimumHorizontalDistance <= 0.0) {
            throw new IllegalArgumentException("minimum horizontal distance must be positive");
        }
        if (minimumVerticalRise < 0.0) {
            throw new IllegalArgumentException("minimum vertical rise must not be negative");
        }

        ScenarioEntityIdentity identity = new ScenarioEntityIdentity(entityId, entityUuid, entityTypeId);
        return waitForEntityMotion(
            entityWaitSource(identity),
            entityId,
            Double.NaN,
            Double.NaN,
            Double.NaN,
            false,
            minimumHorizontalDistance,
            minimumVerticalRise,
            timeout
        );
    }

    static ScenarioEntityMotionObservation waitForEntityMotion(
        EntityWaitSource source,
        int entityId,
        double initialX,
        double initialY,
        double initialZ,
        boolean requireKinematics,
        double minimumHorizontalDistance,
        double minimumVerticalRise,
        Duration timeout
    ) throws Exception {
        Object expectedLevel = source.captureLevel();
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        double startX = initialX;
        double startZ = initialZ;
        double lowestY = initialY;
        double horizontalDistance = 0.0;
        double verticalRise = 0.0;
        double maxHorizontalSpeed = 0.0;
        double minimumYawDelta = 180.0;
        ScenarioEntityMotionObservation latest = null;

        while (true) {
            long observedVersion = source.stateVersion();
            EntityStateSnapshot snapshot = source.snapshot();
            requireSameClientLevel(expectedLevel, snapshot.level());
            EntityMotionSample sample = snapshot.motion();
            if (sample == null) {
                return null;
            }
            if (!Double.isFinite(startX)) {
                startX = sample.x();
                startZ = sample.z();
                lowestY = sample.y();
            }

            horizontalDistance = Math.max(
                horizontalDistance,
                Math.hypot(sample.x() - startX, sample.z() - startZ)
            );
            lowestY = Math.min(lowestY, sample.y());
            verticalRise = Math.max(verticalRise, sample.y() - lowestY);
            maxHorizontalSpeed = Math.max(maxHorizontalSpeed, sample.horizontalSpeed());
            if (Double.isFinite(sample.yawDelta())) {
                minimumYawDelta = Math.min(minimumYawDelta, sample.yawDelta());
            }
            latest = new ScenarioEntityMotionObservation(
                sample.entityTypeId(),
                entityId,
                sample.x(),
                sample.y(),
                sample.z(),
                horizontalDistance,
                verticalRise,
                maxHorizontalSpeed,
                minimumYawDelta
            );
            if (
                horizontalDistance >= minimumHorizontalDistance
                    && verticalRise >= minimumVerticalRise
                    && (!requireKinematics
                        || (maxHorizontalSpeed > 0.0 && minimumYawDelta < 180.0))
            ) {
                return latest;
            }
            if (!source.awaitStateChange(observedVersion, deadlineNanos)) {
                return latest;
            }
        }
    }

    public boolean waitForEntityRemoved(
        int entityId,
        UUID entityUuid,
        String entityTypeId,
        Duration timeout
    ) throws Exception {
        ScenarioEntityIdentity identity = new ScenarioEntityIdentity(entityId, entityUuid, entityTypeId);
        return waitForEntityRemoved(entityWaitSource(identity), timeout);
    }

    static boolean waitForEntityRemoved(EntityWaitSource source, Duration timeout) throws Exception {
        Object expectedLevel = source.captureLevel();
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        while (true) {
            long observedVersion = source.stateVersion();
            EntityStateSnapshot snapshot = source.snapshot();
            requireSameClientLevel(expectedLevel, snapshot.level());
            if (!snapshot.present()) {
                return true;
            }
            if (!source.awaitStateChange(observedVersion, deadlineNanos)) {
                return false;
            }
        }
    }

    private EntityWaitSource entityWaitSource(ScenarioEntityIdentity identity) {
        return new EntityWaitSource() {
            @Override
            public Object captureLevel() throws Exception {
                return executor.callOnClientThread(() -> requireInPlay().level);
            }

            @Override
            public EntityStateSnapshot snapshot() throws Exception {
                return executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    EntityMotionSample motion = entityMotionSampleOnClientThread(identity);
                    return new EntityStateSnapshot(minecraft.level, motion, motion != null);
                });
            }

            @Override
            public long stateVersion() {
                return ClientStateEvents.version();
            }

            @Override
            public boolean awaitStateChange(long observedVersion, long deadlineNanos)
                throws InterruptedException {
                return awaitClientStateChange(observedVersion, deadlineNanos);
            }
        };
    }

    private static void requireSameClientLevel(Object expected, Object observed) {
        if (observed != expected) {
            throw new IllegalStateException("client level changed while waiting for entity state");
        }
    }

    @Override
    public ScenarioPlayerObservation waitForVisiblePlayer(String playerName, Duration timeout) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        ScenarioPlayerObservation finalSample = null;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> visiblePlayerOnClientThread(playerName));
            if (finalSample != null) {
                return finalSample;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public boolean waitForNoVisiblePlayer(String playerName, Duration timeout) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> visiblePlayerOnClientThread(playerName) == null);
            if (finalSample) {
                return true;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public ScenarioPlayerObservation waitForMovedPlayer(
        String playerName,
        ScenarioPlayerObservation baseline,
        double minHorizontalDelta,
        Duration timeout
    ) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        ScenarioPlayerObservation finalSample = null;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> visiblePlayerOnClientThread(playerName));
            if (finalSample != null && horizontalDistance(baseline, finalSample) >= minHorizontalDelta) {
                return finalSample;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return null;
    }

    @Override
    public boolean approachEntity(ScenarioEntityObservation entity, Duration timeout) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        int detourDirection = 0;
        int preferredDetourDirection = 1;
        try {
            do {
                long observedVersion = ClientStateEvents.tickVersion();
                int currentDetourDirection = detourDirection;
                int currentPreferredDirection = preferredDetourDirection;
                EntityApproachSample sample = executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    if (minecraft.screen != null) {
                        minecraft.player.closeContainer();
                        minecraft.setScreen(null);
                    }
                    Entity current = entityByIdOnClientThread(entity.entityId());
                    if (current == null) {
                        minecraft.options.keySprint.setDown(false);
                        minecraft.options.keyUp.setDown(false);
                        minecraft.options.keyJump.setDown(false);
                        minecraft.options.keyLeft.setDown(false);
                        minecraft.options.keyRight.setDown(false);
                        return new EntityApproachSample(false, false, 0);
                    }
                    Vec3 target = entityLookTarget(current);
                    minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, target);
                    MovementClearance clearance = MovementDetour.clearance(minecraft, target);
                    boolean stepUp = minecraft.player.horizontalCollision
                        && clearance.raisedForward();
                    int nextDetourDirection = MovementDetour.choose(
                        currentDetourDirection,
                        currentPreferredDirection,
                        minecraft.player.horizontalCollision && !stepUp,
                        clearance.direct(),
                        clearance.left(),
                        clearance.right()
                    );
                    minecraft.options.keySprint.setDown(true);
                    minecraft.options.keyUp.setDown(nextDetourDirection == 0);
                    minecraft.options.keyJump.setDown(stepUp);
                    minecraft.options.keyLeft.setDown(nextDetourDirection < 0);
                    minecraft.options.keyRight.setDown(nextDetourDirection > 0);
                    boolean inReach = minecraft.player.position().distanceToSqr(current.position())
                        <= SURVIVAL_REACH_SQUARED;
                    return new EntityApproachSample(true, inReach, nextDetourDirection);
                });
                if (detourDirection == 0 && sample.detourDirection() != 0) {
                    preferredDetourDirection = -sample.detourDirection();
                }
                detourDirection = sample.detourDirection();
                if (sample.visible() && sample.inReach()) {
                    return true;
                }
                if (!awaitClientTick(observedVersion, deadlineNanos)) {
                    break;
                }
            } while (true);
            return executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                Entity current = entityByIdOnClientThread(entity.entityId());
                return current != null
                    && minecraft.player.position().distanceToSqr(current.position()) <= SURVIVAL_REACH_SQUARED;
            });
        } finally {
            executor.callOnClientThread(() -> {
                Minecraft minecraft = Minecraft.getInstance();
                if (minecraft.options != null) {
                    minecraft.options.keyUp.setDown(false);
                    minecraft.options.keySprint.setDown(false);
                    minecraft.options.keyJump.setDown(false);
                    minecraft.options.keyLeft.setDown(false);
                    minecraft.options.keyRight.setDown(false);
                }
                return null;
            });
        }
    }

    @Override
    public ScenarioBreakResult attackEntityUntilDropCollected(
        ScenarioEntityObservation entity,
        String expectedDropItemId,
        int expectedSelectedCount,
        Duration timeout
    ) throws Exception {
        int initialCount = executor.callOnClientThread(() -> inventoryCountOnClientThread(expectedDropItemId));
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        long lastAttackTick = -1L;
        boolean started = false;
        boolean removed = false;
        boolean sawDrop = false;
        boolean pickupRestored = false;
        double lastX = entity.x();
        double lastY = entity.y();
        double lastZ = entity.z();
        ScenarioHeldItem selected = selectedItem();
        try {
            do {
                long observedVersion = ClientStateEvents.tickVersion();
                BlockPos near = new BlockPos((int) Math.floor(lastX), (int) Math.floor(lastY), (int) Math.floor(lastZ));
                double capturedLastX = lastX;
                double capturedLastY = lastY;
                double capturedLastZ = lastZ;
                long capturedLastAttackTick = lastAttackTick;
                EntityAttackSample sample = executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    long currentTick = clientTick(minecraft);
                    boolean mayAttack = currentTick != capturedLastAttackTick
                        && AttackCadence.ready(minecraft.player.getAttackStrengthScale(0.0F));
                    Entity current = entityByIdOnClientThread(entity.entityId());
                    Vec3 dropPosition = itemDropPositionOnClientThread(expectedDropItemId, near);
                    if (current != null) {
                        Vec3 target = entityLookTarget(current);
                        minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, target);
                        minecraft.options.keySprint.setDown(
                            minecraft.player.position().distanceToSqr(current.position()) > PICKUP_APPROACH_DISTANCE_SQUARED
                        );
                        minecraft.options.keyUp.setDown(
                            minecraft.player.position().distanceToSqr(current.position()) > PICKUP_APPROACH_DISTANCE_SQUARED
                        );
                        minecraft.options.keyJump.setDown(false);
                        if (mayAttack) {
                            minecraft.gameMode.attack(minecraft.player, current);
                            minecraft.player.swing(InteractionHand.MAIN_HAND);
                        }
                        return new EntityAttackSample(
                            mayAttack,
                            false,
                            dropPosition != null,
                            selectedItemOnClientThread(),
                            inventoryCountOnClientThread(expectedDropItemId),
                            current.getX(),
                            current.getY(),
                            current.getZ(),
                            currentTick
                        );
                    }
                    if (dropPosition != null) {
                        minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, dropPosition);
                        double distanceToDrop = minecraft.player.position().distanceToSqr(dropPosition);
                        MovementClearance clearance = MovementDetour.clearance(minecraft, dropPosition);
                        minecraft.options.keySprint.setDown(distanceToDrop > 0.64);
                        minecraft.options.keyUp.setDown(true);
                        minecraft.options.keyJump.setDown(
                            minecraft.player.horizontalCollision && clearance.raisedForward()
                        );
                    } else {
                        minecraft.options.keySprint.setDown(false);
                        minecraft.options.keyUp.setDown(false);
                        minecraft.options.keyJump.setDown(false);
                    }
                    return new EntityAttackSample(
                        false,
                        true,
                        dropPosition != null,
                        selectedItemOnClientThread(),
                        inventoryCountOnClientThread(expectedDropItemId),
                        dropPosition == null ? capturedLastX : dropPosition.x,
                        dropPosition == null ? capturedLastY : dropPosition.y,
                        dropPosition == null ? capturedLastZ : dropPosition.z,
                        currentTick
                    );
                });
                started |= sample.attackSent();
                removed |= sample.removed();
                sawDrop |= sample.visibleDrop();
                selected = sample.selectedItem();
                lastX = sample.x();
                lastY = sample.y();
                lastZ = sample.z();
                pickupRestored = sample.inventoryCount() >= initialCount + expectedSelectedCount;
                if (sample.attackSent()) {
                    lastAttackTick = sample.tick();
                }
                if (removed && pickupRestored) {
                    return new ScenarioBreakResult(started, true, sawDrop, true, selected);
                }
                if (!awaitClientTick(observedVersion, deadlineNanos)) {
                    break;
                }
            } while (true);
            return new ScenarioBreakResult(started, removed, sawDrop, pickupRestored, selected);
        } finally {
            executor.callOnClientThread(() -> {
                Minecraft minecraft = Minecraft.getInstance();
                if (minecraft.options != null) {
                    minecraft.options.keyUp.setDown(false);
                    minecraft.options.keySprint.setDown(false);
                    minecraft.options.keyJump.setDown(false);
                }
                return null;
            });
        }
    }

    @Override
    public boolean attackEntityUntilRemoved(ScenarioEntityObservation entity, Duration timeout)
        throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        long lastAttackTick = -1L;
        boolean started = false;
        try {
            while (true) {
                long observedVersion = ClientStateEvents.tickVersion();
                long capturedLastAttackTick = lastAttackTick;
                EntityRemovalAttackSample sample = executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    long currentTick = clientTick(minecraft);
                    Entity current = entityByIdOnClientThread(entity.entityId());
                    if (current == null) {
                        return new EntityRemovalAttackSample(false, true, currentTick);
                    }

                    Vec3 target = entityLookTarget(current);
                    minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, target);
                    double distanceSquared = minecraft.player.position().distanceToSqr(current.position());
                    minecraft.options.keySprint.setDown(distanceSquared > PICKUP_APPROACH_DISTANCE_SQUARED);
                    minecraft.options.keyUp.setDown(distanceSquared > PICKUP_APPROACH_DISTANCE_SQUARED);
                    minecraft.options.keyJump.setDown(false);
                    boolean mayAttack = currentTick != capturedLastAttackTick
                        && AttackCadence.ready(minecraft.player.getAttackStrengthScale(0.0F));
                    if (mayAttack) {
                        minecraft.gameMode.attack(minecraft.player, current);
                        minecraft.player.swing(InteractionHand.MAIN_HAND);
                    }
                    return new EntityRemovalAttackSample(mayAttack, false, currentTick);
                });
                started |= sample.attackSent();
                if (sample.attackSent()) {
                    lastAttackTick = sample.tick();
                }
                if (sample.removed()) {
                    return started;
                }
                if (!awaitClientTick(observedVersion, deadlineNanos)) {
                    return false;
                }
            }
        } finally {
            executor.callOnClientThread(() -> {
                Minecraft minecraft = Minecraft.getInstance();
                if (minecraft.options != null) {
                    minecraft.options.keyUp.setDown(false);
                    minecraft.options.keySprint.setDown(false);
                    minecraft.options.keyJump.setDown(false);
                }
                return null;
            });
        }
    }

    @Override
    public boolean drainHungerBySprinting(Duration timeout) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        List<int[]> waypoints = executor.callOnClientThread(
            MinecraftScenarioClient::safeHungerDrainWaypointsOnClientThread
        );
        if (waypoints.isEmpty()) {
            return false;
        }
        int nextWaypoint = 0;
        try {
            while (true) {
                boolean foodBelowFull = executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    return minecraft.player.getFoodData().getFoodLevel() < 20;
                });
                if (foodBelowFull) {
                    return true;
                }

                long remainingNanos = deadlineNanos - System.nanoTime();
                if (remainingNanos <= 0L) {
                    return false;
                }
                long legNanos = Math.min(remainingNanos, Duration.ofSeconds(12).toNanos());
                int[] waypoint = waypoints.get(nextWaypoint);
                boolean reached = approachPosition(
                    waypoint[0],
                    waypoint[1],
                    Duration.ofNanos(legNanos)
                );
                if (reached) {
                    nextWaypoint = (nextWaypoint + 1) % waypoints.size();
                } else {
                    waypoints.remove(nextWaypoint);
                    if (waypoints.isEmpty()) {
                        return false;
                    }
                    nextWaypoint %= waypoints.size();
                }
            }
        } finally {
            executor.callOnClientThread(() -> {
                Minecraft minecraft = Minecraft.getInstance();
                if (minecraft.options != null) {
                    minecraft.options.keyUp.setDown(false);
                    minecraft.options.keySprint.setDown(false);
                    minecraft.options.keyJump.setDown(false);
                }
                return null;
            });
        }
    }

    @Override
    public ScenarioFoodUseResult eatSelectedFood(String itemId, int itemCountBefore, Duration timeout)
        throws Exception {
        FoodUseSample start = executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            minecraft.options.keyUse.setDown(true);
            InteractionResult result = minecraft.gameMode.useItem(minecraft.player, InteractionHand.MAIN_HAND);
            return new FoodUseSample(
                !"PASS".equals(result.toString()),
                minecraft.player.getFoodData().getFoodLevel(),
                inventoryCountOnClientThread(itemId)
            );
        });
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        FoodUseSample latest = start;
        try {
            do {
                long observedVersion = ClientStateEvents.tickVersion();
                latest = executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    minecraft.options.keyUse.setDown(true);
                    return new FoodUseSample(
                        minecraft.player.isUsingItem(),
                        minecraft.player.getFoodData().getFoodLevel(),
                        inventoryCountOnClientThread(itemId)
                    );
                });
                if (latest.foodLevel() > start.foodLevel() && latest.itemCount() < itemCountBefore) {
                    return new ScenarioFoodUseResult(
                        start.started(),
                        start.foodLevel(),
                        latest.foodLevel(),
                        itemCountBefore,
                        latest.itemCount()
                    );
                }
                if (!awaitClientTick(observedVersion, deadlineNanos)) {
                    break;
                }
            } while (true);
            return new ScenarioFoodUseResult(
                start.started(),
                start.foodLevel(),
                latest.foodLevel(),
                itemCountBefore,
                latest.itemCount()
            );
        } finally {
            executor.callOnClientThread(() -> {
                Minecraft minecraft = Minecraft.getInstance();
                if (minecraft.options != null) {
                    minecraft.options.keyUse.setDown(false);
                }
                return null;
            });
        }
    }

    @Override
    public ScenarioShieldBlockResult blockAttackWithSelectedShield(String itemId, Duration timeout)
        throws Exception {
        ShieldBlockSample initial = executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            ItemStack selected = minecraft.player.getInventory().getSelectedItem();
            float health = minecraft.player.getHealth();
            if (!itemStackMatches(selected, itemId, 1)) {
                return new ShieldBlockSample(false, health, selected.getDamageValue());
            }
            minecraft.options.keyUse.setDown(true);
            InteractionResult result = minecraft.gameMode.useItem(minecraft.player, InteractionHand.MAIN_HAND);
            boolean started = !"PASS".equals(result.toString()) || minecraft.player.isUsingItem();
            return new ShieldBlockSample(started, health, selected.getDamageValue());
        });
        if (!initial.useStarted()) {
            return new ScenarioShieldBlockResult(
                false,
                false,
                initial.health(),
                initial.health(),
                initial.shieldDamage(),
                initial.shieldDamage()
            );
        }

        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        try {
            do {
                long observedVersion = ClientStateEvents.version();
                ShieldBlockSample sample = executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    minecraft.options.keyUse.setDown(true);
                    ItemStack selected = minecraft.player.getInventory().getSelectedItem();
                    return new ShieldBlockSample(
                        itemStackMatches(selected, itemId, 1) && minecraft.player.isUsingItem(),
                        minecraft.player.getHealth(),
                        selected.getDamageValue()
                    );
                });
                boolean blockedAttackObserved = sample.shieldDamage() > initial.shieldDamage();
                if (blockedAttackObserved || sample.health() < initial.health() || !sample.useStarted()) {
                    return new ScenarioShieldBlockResult(
                        true,
                        blockedAttackObserved,
                        initial.health(),
                        sample.health(),
                        initial.shieldDamage(),
                        sample.shieldDamage()
                    );
                }
                if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                    return new ScenarioShieldBlockResult(
                        true,
                        false,
                        initial.health(),
                        sample.health(),
                        initial.shieldDamage(),
                        sample.shieldDamage()
                    );
                }
            } while (true);
        } finally {
            executor.callOnClientThread(() -> {
                Minecraft minecraft = Minecraft.getInstance();
                if (minecraft.options != null) {
                    minecraft.options.keyUse.setDown(false);
                }
                return null;
            });
        }
    }

    @Override
    public boolean quickEquipSelectedArmor(String itemId, String armorSlot, Duration duration)
        throws Exception {
        int armorMenuSlot = armorMenuSlot(armorSlot);
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(
                () -> containerSlotMatchesOnClientThread(armorMenuSlot, itemId, 1)
            );
            if (finalSample) {
                return true;
            }
            executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                if (minecraft.screen != null) {
                    minecraft.player.closeContainer();
                    minecraft.setScreen(null);
                }
                AbstractContainerMenu menu = minecraft.player.containerMenu;
                if (
                    armorMenuSlot < menu.slots.size()
                        && itemStackMatches(menu.getSlot(armorMenuSlot).getItem(), itemId, 1)
                ) {
                    return null;
                }
                if (!selectedItemOnClientThread().matches(itemId, 1)) {
                    return null;
                }
                int sourceSlot = findMenuSlotWithItem(menu, itemId, 1, armorMenuSlot);
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
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return executor.callOnClientThread(
            () -> containerSlotMatchesOnClientThread(armorMenuSlot, itemId, 1)
        );
    }

    @Override
    public ScenarioHeldItem equippedArmor(String armorSlot) throws Exception {
        int armorMenuSlot = armorMenuSlot(armorSlot);
        return executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            AbstractContainerMenu menu = minecraft.player.containerMenu;
            if (armorMenuSlot >= menu.slots.size()) {
                return new ScenarioHeldItem("minecraft:air", 0);
            }
            return heldItemFromStack(menu.getSlot(armorMenuSlot).getItem());
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
            long observedVersion = ClientStateEvents.version();
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
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return finalSample;
    }

    @Override
    public boolean waitForDeathScreen(Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                return minecraft.screen instanceof DeathScreen;
            });
            if (finalSample) {
                return true;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
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
            long observedVersion = ClientStateEvents.version();
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
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return false;
    }

    private BlockBreakAutomation startBlockBreakAfterReset(
        ScenarioBlockTarget target,
        Direction face,
        long deadlineNanos
    ) throws Exception {
        BlockBreakAutomation action = new BlockBreakAutomation(target, face);
        executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            BlockBreakAutomation previous = ACTIVE_BLOCK_BREAK.getAndSet(null);
            if (previous != null && !previous.started.isDone()) {
                previous.started.completeExceptionally(
                    new IllegalStateException("block break replaced before it started")
                );
            }
            minecraft.options.keyAttack.setDown(false);
            minecraft.gameMode.stopDestroyBlock();
            ACTIVE_BLOCK_BREAK.set(action);
            return null;
        });
        if (!awaitBlockBreakStarted(action, deadlineNanos)) {
            stopBlockBreak(action);
            return null;
        }
        return action;
    }

    private static boolean awaitBlockBreakStarted(
        BlockBreakAutomation action,
        long deadlineNanos
    ) throws Exception {
        long remainingNanos = deadlineNanos - System.nanoTime();
        if (remainingNanos <= 0L) {
            return false;
        }
        try {
            action.started.get(remainingNanos, TimeUnit.NANOSECONDS);
            return true;
        } catch (TimeoutException error) {
            return false;
        } catch (ExecutionException error) {
            Throwable cause = error.getCause();
            if (cause instanceof Exception exception) {
                throw exception;
            }
            throw new IllegalStateException("block break failed before start", cause);
        }
    }

    private void stopBlockBreak(BlockBreakAutomation action) throws Exception {
        executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            ACTIVE_BLOCK_BREAK.compareAndSet(action, null);
            minecraft.options.keyAttack.setDown(false);
            minecraft.gameMode.stopDestroyBlock();
            return null;
        });
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
        BlockBreakAutomation action = startBlockBreakAfterReset(target, face, deadlineNanos);
        if (action == null) {
            return new ScenarioBreakResult(false, false, false, false, selectedItem());
        }

        boolean sawDrop = false;
        boolean becameAir = false;
        boolean breakStopped = false;
        ScenarioHeldItem selected = selectedItem();
        try {
            do {
                long observedVersion = ClientStateEvents.tickVersion();
                BreakSample sample = executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    return new BreakSample(
                        minecraft.level.getBlockState(pos(target)).isAir(),
                        itemDropVisibleOnClientThread(expectedDropItemId, pos(target)),
                        selectedItemOnClientThread(),
                        clientTick(minecraft)
                    );
                });
                sawDrop |= sample.sawDrop();
                becameAir |= sample.becameAir();
                selected = sample.selectedItem();
                if (becameAir && !breakStopped) {
                    breakStopped = true;
                    stopBlockBreak(action);
                }
                if (becameAir && selected.matches(expectedDropItemId, expectedSelectedCount)) {
                    return new ScenarioBreakResult(true, true, sawDrop, true, selected);
                }
                if (!awaitClientTick(observedVersion, deadlineNanos)) {
                    break;
                }
            } while (true);
            return new ScenarioBreakResult(
                true,
                becameAir,
                sawDrop,
                selected.matches(expectedDropItemId, expectedSelectedCount),
                selected
            );
        } finally {
            if (!breakStopped) {
                stopBlockBreak(action);
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
        BlockBreakAutomation action = startBlockBreakAfterReset(target, face, deadlineNanos);
        if (action == null) {
            return new ScenarioBreakResult(false, false, false, false, selectedItem());
        }

        boolean sawDrop = false;
        boolean becameAir = false;
        boolean breakStopped = false;
        ScenarioHeldItem selected = selectedItem();
        try {
            do {
                long observedVersion = ClientStateEvents.tickVersion();
                BreakSample sample = executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    return new BreakSample(
                        minecraft.level.getBlockState(pos(target)).isAir(),
                        itemDropVisibleOnClientThread(expectedDropItemId, pos(target)),
                        selectedItemOnClientThread(),
                        clientTick(minecraft)
                    );
                });
                sawDrop |= sample.sawDrop();
                becameAir |= sample.becameAir();
                selected = sample.selectedItem();
                if (becameAir && !breakStopped) {
                    breakStopped = true;
                    stopBlockBreak(action);
                }
                if (becameAir && sawDrop) {
                    return new ScenarioBreakResult(true, true, true, false, selected);
                }
                if (!awaitClientTick(observedVersion, deadlineNanos)) {
                    break;
                }
            } while (true);
            return new ScenarioBreakResult(true, becameAir, sawDrop, false, selected);
        } finally {
            if (!breakStopped) {
                stopBlockBreak(action);
            }
        }
    }

    @Override
    public boolean waitForVisibleItemDrop(String itemId, ScenarioBlockTarget near, Duration timeout)
        throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        boolean finalSample = false;
        do {
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> itemDropVisibleOnClientThread(itemId, pos(near)));
            if (finalSample) {
                return true;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return false;
    }

    @Override
    public List<ScenarioItemDropIdentity> visibleItemDropIdentities(String itemId) throws Exception {
        return executor.callOnClientThread(() -> itemDropIdentitiesOnClientThread(itemId));
    }

    @Override
    public ScenarioItemDropIdentity waitForNewVisibleItemDropIdentity(
        String itemId,
        List<ScenarioItemDropIdentity> excludedIdentities,
        Duration timeout
    ) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        do {
            long observedVersion = ClientStateEvents.version();
            ScenarioItemDropIdentity identity = executor.callOnClientThread(
                () -> newItemDropIdentityOnClientThread(itemId, excludedIdentities)
            );
            if (identity != null) {
                return identity;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                return null;
            }
        } while (true);
    }

    @Override
    public ScenarioBreakResult collectVisibleItemDrop(
        ScenarioBlockTarget near,
        String expectedDropItemId,
        int expectedSelectedCount,
        Duration timeout
    ) throws Exception {
        return collectVisibleItemDrop(near, expectedDropItemId, null, expectedSelectedCount, timeout);
    }

    @Override
    public ScenarioBreakResult collectVisibleItemDropByIdentity(
        ScenarioBlockTarget near,
        String expectedDropItemId,
        ScenarioItemDropIdentity expectedIdentity,
        int expectedSelectedCount,
        Duration timeout
    ) throws Exception {
        return collectVisibleItemDrop(
            near,
            expectedDropItemId,
            expectedIdentity,
            expectedSelectedCount,
            timeout
        );
    }

    private ScenarioBreakResult collectVisibleItemDrop(
        ScenarioBlockTarget near,
        String expectedDropItemId,
        ScenarioItemDropIdentity expectedIdentity,
        int expectedSelectedCount,
        Duration timeout
    ) throws Exception {
        int initialCount = executor.callOnClientThread(() -> inventoryCountOnClientThread(expectedDropItemId));
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        int detourDirection = 0;
        int preferredDetourDirection = 1;
        boolean sawDrop = false;
        boolean dropGone = false;
        boolean pickupRestored = false;
        boolean itemTakeObserved = false;
        ScenarioHeldItem selected = selectedItem();
        PickupSample latestSample = null;
        try {
            do {
                long observedTickVersion = ClientStateEvents.tickVersion();
                long observedStateVersion = ClientStateEvents.version();
                int currentDetourDirection = detourDirection;
                int currentPreferredDirection = preferredDetourDirection;
                PickupSample sample = executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    Vec3 center = Vec3.atLowerCornerWithOffset(pos(near), 0.5, 0.5, 0.5);
                    Vec3 dropPosition = expectedIdentity == null
                        ? itemDropPositionOnClientThread(expectedDropItemId, pos(near))
                        : itemDropPositionOnClientThread(expectedDropItemId, expectedIdentity);
                    boolean identityTaken = expectedIdentity != null
                        && ClientStateEvents.consumeItemTakenBy(expectedIdentity, minecraft.player.getId());
                    if (expectedIdentity != null && dropPosition == null) {
                        minecraft.options.keyUp.setDown(false);
                        minecraft.options.keySprint.setDown(false);
                        minecraft.options.keyJump.setDown(false);
                        minecraft.options.keyLeft.setDown(false);
                        minecraft.options.keyRight.setDown(false);
                        Vec3 playerPosition = minecraft.player.position();
                        return new PickupSample(
                            false,
                            selectedItemOnClientThread(),
                            inventoryCountOnClientThread(expectedDropItemId),
                            playerPosition.x,
                            playerPosition.y,
                            playerPosition.z,
                            Double.NaN,
                            Double.NaN,
                            Double.NaN,
                            Double.NaN,
                            minecraft.player.horizontalCollision,
                            minecraft.player.onGround(),
                            0,
                            clientTick(minecraft),
                            identityTaken
                        );
                    }
                    Vec3 target = dropPosition == null ? center : dropPosition;
                    clearFoliageObstacleTowardOnClientThread(near);
                    minecraft.player.lookAt(
                        EntityAnchorArgument.Anchor.EYES,
                        target
                    );
                    MovementClearance clearance = MovementDetour.clearance(minecraft, target);
                    boolean stepUp = minecraft.player.horizontalCollision
                        && clearance.raisedForward();
                    int nextDetourDirection = MovementDetour.choose(
                        currentDetourDirection,
                        currentPreferredDirection,
                        minecraft.player.horizontalCollision && !stepUp,
                        clearance.direct(),
                        clearance.left(),
                        clearance.right()
                    );
                    minecraft.options.keySprint.setDown(true);
                    minecraft.options.keyUp.setDown(nextDetourDirection == 0);
                    minecraft.options.keyJump.setDown(stepUp);
                    minecraft.options.keyLeft.setDown(nextDetourDirection < 0);
                    minecraft.options.keyRight.setDown(nextDetourDirection > 0);
                    boolean visible = dropPosition != null;
                    Vec3 playerPosition = minecraft.player.position();
                    return new PickupSample(
                        visible,
                        selectedItemOnClientThread(),
                        inventoryCountOnClientThread(expectedDropItemId),
                        playerPosition.x,
                        playerPosition.y,
                        playerPosition.z,
                        dropPosition == null ? Double.NaN : dropPosition.x,
                        dropPosition == null ? Double.NaN : dropPosition.y,
                        dropPosition == null ? Double.NaN : dropPosition.z,
                        dropPosition == null ? Double.NaN : playerPosition.distanceToSqr(dropPosition),
                        minecraft.player.horizontalCollision,
                        minecraft.player.onGround(),
                        nextDetourDirection,
                        clientTick(minecraft),
                        identityTaken
                    );
                });
                latestSample = sample;
                itemTakeObserved |= sample.identityTaken();
                if (detourDirection == 0 && sample.detourDirection() != 0) {
                    preferredDetourDirection = -sample.detourDirection();
                }
                detourDirection = sample.detourDirection();
                sawDrop |= sample.visibleDrop();
                dropGone = !sample.visibleDrop();
                selected = sample.selectedItem();
                pickupRestored = expectedIdentity == null
                    ? sample.inventoryCount() >= initialCount + expectedSelectedCount
                    : itemTakeObserved
                        && sample.inventoryCount() == initialCount + expectedSelectedCount;
                if (dropGone && pickupRestored) {
                    return new ScenarioBreakResult(
                        true,
                        true,
                        sawDrop,
                        true,
                        selected,
                        pickupDetail(sample)
                    );
                }
                if (expectedIdentity != null && dropGone && !itemTakeObserved) {
                    return new ScenarioBreakResult(
                        true,
                        true,
                        sawDrop,
                        false,
                        selected,
                        pickupDetail(sample)
                    );
                }
                if (expectedIdentity != null && dropGone) {
                    if (!awaitClientStateChange(observedStateVersion, deadlineNanos)) {
                        break;
                    }
                    continue;
                }
                if (!awaitClientTick(observedTickVersion, deadlineNanos)) {
                    break;
                }
            } while (true);
            return new ScenarioBreakResult(
                true,
                dropGone,
                sawDrop,
                pickupRestored,
                selected,
                pickupDetail(latestSample)
            );
        } finally {
            executor.callOnClientThread(() -> {
                Minecraft minecraft = Minecraft.getInstance();
                if (minecraft.options != null) {
                    minecraft.options.keyUp.setDown(false);
                    minecraft.options.keySprint.setDown(false);
                    minecraft.options.keyJump.setDown(false);
                    minecraft.options.keyLeft.setDown(false);
                    minecraft.options.keyRight.setDown(false);
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
            long observedVersion = ClientStateEvents.version();
            finalSample = executor.callOnClientThread(() -> itemDropVisibleOnClientThread(itemId, pos(near)));
            if (!finalSample) {
                return true;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return !finalSample;
    }

    @Override
    public ScenarioBlockTarget findBreakableBlock(List<String> blockIds, ScenarioReach reach) throws Exception {
        return executor.callOnClientThread(() -> findBreakableBlockOnClientThread(blockIds, reach));
    }

    @Override
    public boolean approachBlock(ScenarioBlockTarget target, Duration timeout) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        int detourDirection = 0;
        int preferredDetourDirection = 1;
        try {
            do {
                long observedVersion = ClientStateEvents.tickVersion();
                int currentDetourDirection = detourDirection;
                int currentPreferredDirection = preferredDetourDirection;
                ApproachSample sample = executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    if (minecraft.screen != null) {
                        minecraft.player.closeContainer();
                        minecraft.setScreen(null);
                    }
                    Vec3 center = Vec3.atLowerCornerWithOffset(pos(target), 0.5, 0.5, 0.5);
                    minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, center);
                    clearFoliageObstacleTowardOnClientThread(target);
                    MovementClearance clearance = MovementDetour.clearance(minecraft, center);
                    boolean stepUp = minecraft.player.horizontalCollision
                        && clearance.raisedForward();
                    int nextDetourDirection = MovementDetour.choose(
                        currentDetourDirection,
                        currentPreferredDirection,
                        minecraft.player.horizontalCollision && !stepUp,
                        clearance.direct(),
                        clearance.left(),
                        clearance.right()
                    );
                    minecraft.options.keySprint.setDown(true);
                    minecraft.options.keyUp.setDown(nextDetourDirection == 0);
                    minecraft.options.keyJump.setDown(stepUp);
                    minecraft.options.keyLeft.setDown(nextDetourDirection < 0);
                    minecraft.options.keyRight.setDown(nextDetourDirection > 0);
                    boolean targetInReach = breakableBlockTarget(
                        pos(target),
                        minecraft.player.getEyePosition(),
                        ScenarioReach.WITHIN_SURVIVAL_REACH,
                        List.of(target.blockId())
                    ) != null;
                    return new ApproachSample(targetInReach, nextDetourDirection);
                });
                if (detourDirection == 0 && sample.detourDirection() != 0) {
                    preferredDetourDirection = -sample.detourDirection();
                }
                detourDirection = sample.detourDirection();
                if (sample.inReach()) {
                    return true;
                }
                if (!awaitClientTick(observedVersion, deadlineNanos)) {
                    break;
                }
            } while (true);
            return executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                boolean inReach = breakableBlockTarget(
                    pos(target),
                    minecraft.player.getEyePosition(),
                    ScenarioReach.WITHIN_SURVIVAL_REACH,
                    List.of(target.blockId())
                ) != null;
                return inReach;
            });
        } finally {
            executor.callOnClientThread(() -> {
                Minecraft minecraft = Minecraft.getInstance();
                if (minecraft.options != null) {
                    minecraft.options.keyUp.setDown(false);
                    minecraft.options.keySprint.setDown(false);
                    minecraft.options.keyJump.setDown(false);
                    minecraft.options.keyLeft.setDown(false);
                    minecraft.options.keyRight.setDown(false);
                }
                return null;
            });
        }
    }

    @Override
    public boolean approachPosition(int x, int z, Duration timeout) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        Vec3 target = new Vec3(x + 0.5, 0.0, z + 0.5);
        int detourDirection = 0;
        int preferredDetourDirection = 1;
        try {
            do {
                long observedVersion = ClientStateEvents.tickVersion();
                int currentDetourDirection = detourDirection;
                int currentPreferredDirection = preferredDetourDirection;
                ApproachSample sample = executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    if (minecraft.screen != null) {
                        minecraft.player.closeContainer();
                        minecraft.setScreen(null);
                    }
                    Vec3 playerPosition = minecraft.player.position();
                    Vec3 horizontalTarget = new Vec3(target.x, playerPosition.y, target.z);
                    minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, horizontalTarget);
                    MovementClearance clearance = MovementDetour.clearance(minecraft, horizontalTarget);
                    boolean stepUp = minecraft.player.horizontalCollision
                        && clearance.raisedForward();
                    int nextDetourDirection = MovementDetour.choose(
                        currentDetourDirection,
                        currentPreferredDirection,
                        minecraft.player.horizontalCollision && !stepUp,
                        clearance.direct(),
                        clearance.left(),
                        clearance.right()
                    );
                    minecraft.options.keySprint.setDown(true);
                    minecraft.options.keyUp.setDown(nextDetourDirection == 0);
                    minecraft.options.keyJump.setDown(stepUp);
                    minecraft.options.keyLeft.setDown(nextDetourDirection < 0);
                    minecraft.options.keyRight.setDown(nextDetourDirection > 0);
                    double deltaX = minecraft.player.getX() - target.x;
                    double deltaZ = minecraft.player.getZ() - target.z;
                    return new ApproachSample(
                        deltaX * deltaX + deltaZ * deltaZ <= POSITION_APPROACH_DISTANCE_SQUARED,
                        nextDetourDirection
                    );
                });
                if (detourDirection == 0 && sample.detourDirection() != 0) {
                    preferredDetourDirection = -sample.detourDirection();
                }
                detourDirection = sample.detourDirection();
                if (sample.inReach()) {
                    return true;
                }
                if (!awaitClientTick(observedVersion, deadlineNanos)) {
                    break;
                }
            } while (true);
            return executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                double deltaX = minecraft.player.getX() - target.x;
                double deltaZ = minecraft.player.getZ() - target.z;
                return deltaX * deltaX + deltaZ * deltaZ <= POSITION_APPROACH_DISTANCE_SQUARED;
            });
        } finally {
            executor.callOnClientThread(() -> {
                Minecraft minecraft = Minecraft.getInstance();
                if (minecraft.options != null) {
                    minecraft.options.keyUp.setDown(false);
                    minecraft.options.keySprint.setDown(false);
                    minecraft.options.keyJump.setDown(false);
                    minecraft.options.keyLeft.setDown(false);
                    minecraft.options.keyRight.setDown(false);
                }
                return null;
            });
        }
    }

    @Override
    public ScenarioBlockTarget findLoadedBlockInColumn(int x, int z, List<String> blockIds) throws Exception {
        return executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            if (!minecraft.level.hasChunk(x >> 4, z >> 4)) {
                return null;
            }
            Vec3 eye = minecraft.player.getEyePosition();
            for (int y = minecraft.level.getMinY(); y < minecraft.level.getMaxY(); y++) {
                BlockPos target = new BlockPos(x, y, z);
                if (!minecraft.level.isLoaded(target)) {
                    continue;
                }
                String blockId = blockIdAt(target);
                if (!blockIds.contains(blockId)) {
                    continue;
                }
                Direction face = closestAccessibleFace(target, eye);
                if (face == null) {
                    continue;
                }
                return new ScenarioBlockTarget(x, y, z, face.getName(), "loaded-column", blockId);
            }
            return null;
        });
    }

    @Override
    public boolean standOnBlockUntilDeath(ScenarioBlockTarget target, Duration timeout) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        int detourDirection = 0;
        int preferredDetourDirection = 1;
        try {
            do {
                long observedVersion = ClientStateEvents.tickVersion();
                int currentDetourDirection = detourDirection;
                int currentPreferredDirection = preferredDetourDirection;
                HazardStandSample sample = executor.callOnClientThread(() -> {
                    Minecraft minecraft = requireInPlay();
                    if (minecraft.screen instanceof DeathScreen) {
                        return new HazardStandSample(true, 0);
                    }
                    if (minecraft.screen != null) {
                        minecraft.player.closeContainer();
                        minecraft.setScreen(null);
                    }
                    BlockPos targetPos = pos(target);
                    Vec3 topCenter = Vec3.atLowerCornerWithOffset(targetPos, 0.5, 1.0, 0.5);
                    double dx = minecraft.player.getX() - topCenter.x;
                    double dz = minecraft.player.getZ() - topCenter.z;
                    double distanceSquared = dx * dx + dz * dz;
                    boolean highEnough = minecraft.player.getY() >= targetPos.getY() - 0.05;
                    boolean onTarget = distanceSquared <= 0.08 && highEnough;
                    minecraft.player.lookAt(EntityAnchorArgument.Anchor.EYES, topCenter);
                    MovementClearance clearance = MovementDetour.clearance(minecraft, topCenter);
                    boolean stepUp = !onTarget
                        && minecraft.player.horizontalCollision
                        && clearance.raisedForward();
                    int nextDetourDirection = MovementDetour.choose(
                        currentDetourDirection,
                        currentPreferredDirection,
                        minecraft.player.horizontalCollision && !stepUp,
                        clearance.direct(),
                        clearance.left(),
                        clearance.right()
                    );
                    minecraft.options.keySprint.setDown(!onTarget);
                    minecraft.options.keyUp.setDown(!onTarget && nextDetourDirection == 0);
                    minecraft.options.keyJump.setDown(stepUp);
                    minecraft.options.keyLeft.setDown(!onTarget && nextDetourDirection < 0);
                    minecraft.options.keyRight.setDown(!onTarget && nextDetourDirection > 0);
                    return new HazardStandSample(false, nextDetourDirection);
                });
                if (detourDirection == 0 && sample.detourDirection() != 0) {
                    preferredDetourDirection = -sample.detourDirection();
                }
                detourDirection = sample.detourDirection();
                if (sample.dead()) {
                    return true;
                }
                if (!awaitClientTick(observedVersion, deadlineNanos)) {
                    break;
                }
            } while (true);
            return executor.callOnClientThread(() -> Minecraft.getInstance().screen instanceof DeathScreen);
        } finally {
            executor.callOnClientThread(() -> {
                Minecraft minecraft = Minecraft.getInstance();
                if (minecraft.options != null) {
                    minecraft.options.keyUp.setDown(false);
                    minecraft.options.keySprint.setDown(false);
                    minecraft.options.keyJump.setDown(false);
                    minecraft.options.keyLeft.setDown(false);
                    minecraft.options.keyRight.setDown(false);
                }
                return null;
            });
        }
    }

    @Override
    public ScenarioHeldItem selectHotbarItem(String itemId, int count, Duration timeout) throws Exception {
        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        ScenarioHeldItem latest;
        do {
            long observedVersion = ClientStateEvents.version();
            HotbarSelectionAttempt attempt = executor.callOnClientThread(
                () -> selectHotbarItemOnClientThread(itemId, count)
            );
            latest = attempt.selectedItem();
            if (attempt.checkpoint() == null) {
                if (latest.matches(itemId, count)) {
                    return latest;
                }
                if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                    break;
                }
                continue;
            }

            HotbarSelectionResponse response = waitForAuthoritativeHotbarSelection(
                attempt,
                itemId,
                count,
                deadlineNanos
            );
            latest = response.selectedItem();
            if (response.confirmed()) {
                return latest;
            }
            if (!response.observed()) {
                break;
            }
        } while (true);
        return latest;
    }

    private HotbarSelectionResponse waitForAuthoritativeHotbarSelection(
        HotbarSelectionAttempt attempt,
        String itemId,
        int count,
        long deadlineNanos
    ) throws Exception {
        HotbarSelectionResponse latest;
        do {
            long observedVersion = ClientStateEvents.version();
            latest = executor.callOnClientThread(() -> {
                Minecraft minecraft = requireInPlay();
                AbstractContainerMenu menu = minecraft.player.containerMenu;
                ScenarioHeldItem selectedItem = selectedItemOnClientThread();
                boolean slotMatches = minecraft.player.getInventory().getSelectedSlot()
                        == attempt.targetHotbarSlot()
                    && itemStackMatches(
                        minecraft.player.getInventory().getItem(attempt.targetHotbarSlot()),
                        itemId,
                        count
                    );
                boolean confirmed = ScenarioClient.authoritativeContainerUpdateMatches(
                    attempt.checkpoint().containerId(),
                    attempt.checkpoint().stateId(),
                    menu.containerId,
                    menu.getStateId(),
                    slotMatches
                );
                boolean observed = menu.containerId == attempt.checkpoint().containerId()
                    && menu.getStateId() != attempt.checkpoint().stateId();
                return new HotbarSelectionResponse(observed, confirmed, selectedItem);
            });
            if (latest.observed() || !awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return latest;
    }

    @Override
    public ScenarioBlockTarget dropSelectedItem(String itemId, int count, Duration timeout) throws Exception {
        int initialCount = executor.callOnClientThread(() -> inventoryCountOnClientThread(itemId));
        ScenarioBlockTarget target = executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            ScenarioHeldItem selected = selectedItemOnClientThread();
            if (!selected.matches(itemId, count)) {
                throw new IllegalStateException(
                    "selected item does not match drop request expected="
                        + itemId
                        + " x"
                        + count
                        + " selected="
                        + selected.itemId()
                        + " x"
                        + selected.count()
                );
            }
            BlockPos origin = minecraft.player.blockPosition();
            boolean dropped = minecraft.player.drop(false);
            minecraft.player.swing(InteractionHand.MAIN_HAND);
            if (!dropped) {
                throw new IllegalStateException("client refused selected item drop for " + itemId);
            }
            return new ScenarioBlockTarget(
                origin.getX(),
                origin.getY(),
                origin.getZ(),
                "up",
                "selected-item-drop",
                itemId
            );
        });

        long deadlineNanos = System.nanoTime() + timeout.toNanos();
        int expectedCount = Math.max(0, initialCount - count);
        int latestCount;
        do {
            long observedVersion = ClientStateEvents.version();
            latestCount = executor.callOnClientThread(() -> inventoryCountOnClientThread(itemId));
            if (latestCount <= expectedCount) {
                return target;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return target;
    }

    @Override
    public ScenarioHeldItem selectedItem() throws Exception {
        return executor.callOnClientThread(MinecraftScenarioClient::selectedItemOnClientThread);
    }

    @Override
    public float playerHealth() throws Exception {
        return executor.callOnClientThread(() -> {
            Minecraft minecraft = requireInPlay();
            return minecraft.player.getHealth();
        });
    }

    @Override
    public float waitForPlayerHealthBelow(float health, Duration duration) throws Exception {
        long deadlineNanos = System.nanoTime() + duration.toNanos();
        float latest;
        do {
            long observedVersion = ClientStateEvents.version();
            latest = playerHealth();
            if (latest < health - 0.001F) {
                return latest;
            }
            if (!awaitClientStateChange(observedVersion, deadlineNanos)) {
                break;
            }
        } while (true);
        return latest;
    }

    static void selectHotbarSlotOnClientThread(int slot) {
        Minecraft minecraft = requireInPlay();
        if (slot < 0 || slot > 8) {
            throw new IllegalArgumentException("hotbar slot must be 0..8");
        }
        minecraft.player.getInventory().setSelectedSlot(slot);
        minecraft.getConnection().send(new ServerboundSetCarriedItemPacket(slot));
    }

    private static HotbarSelectionAttempt selectHotbarItemOnClientThread(String itemId, int count) {
        Minecraft minecraft = requireInPlay();
        for (int slot = 0; slot <= 8; slot++) {
            ItemStack stack = minecraft.player.getInventory().getItem(slot);
            if (itemStackMatches(stack, itemId, count)) {
                selectHotbarSlotOnClientThread(slot);
                return new HotbarSelectionAttempt(selectedItemOnClientThread(), null, slot);
            }
        }
        AbstractContainerMenu menu = minecraft.player.containerMenu;
        int sourceSlot = findMenuSlotWithItem(menu, itemId, count, -1);
        if (sourceSlot >= 0) {
            int targetHotbarSlot = preferredHotbarSwapSlotOnClientThread();
            ContainerUpdateCheckpoint checkpoint = new ContainerUpdateCheckpoint(
                menu.containerId,
                menu.getStateId()
            );
            minecraft.gameMode.handleContainerInput(
                menu.containerId,
                sourceSlot,
                targetHotbarSlot,
                ContainerInput.SWAP,
                minecraft.player
            );
            selectHotbarSlotOnClientThread(targetHotbarSlot);
            return new HotbarSelectionAttempt(
                selectedItemOnClientThread(),
                checkpoint,
                targetHotbarSlot
            );
        }
        return new HotbarSelectionAttempt(selectedItemOnClientThread(), null, -1);
    }

    private static int preferredHotbarSwapSlotOnClientThread() {
        Minecraft minecraft = requireInPlay();
        int selected = minecraft.player.getInventory().getSelectedSlot();
        if (minecraft.player.getInventory().getItem(selected).isEmpty()) {
            return selected;
        }
        for (int slot = 0; slot <= 8; slot++) {
            if (minecraft.player.getInventory().getItem(slot).isEmpty()) {
                return slot;
            }
        }
        return selected;
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
        for (int radius = 1; radius <= NATURAL_BREAKABLE_SCAN_RADIUS; radius++) {
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
        return findPlaceablePairOnClientThread(reach, requireDryTarget, false);
    }

    private static ScenarioBlockPair findPlaceablePairOnClientThread(
        ScenarioReach reach,
        boolean requireDryTarget,
        boolean requirePlayerClearance
    ) {
        return findPlaceablePairOnClientThread(
            reach,
            requireDryTarget,
            requirePlayerClearance,
            PLACE_DIRECTIONS
        );
    }

    private static ScenarioBlockPair findPlaceablePairOnClientThread(
        ScenarioReach reach,
        boolean requireDryTarget,
        boolean requirePlayerClearance,
        Direction[] directions
    ) {
        Minecraft minecraft = requireInPlay();
        BlockPos origin = minecraft.player.blockPosition();
        Vec3 eye = minecraft.player.getEyePosition();
        for (int radius = 1; radius <= NATURAL_BREAKABLE_SCAN_RADIUS; radius++) {
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
                            requireDryTarget,
                            requirePlayerClearance,
                            directions
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

    private static ScenarioBlockPair findHorizontalAttachmentPairOnClientThread(
        ScenarioBlockTarget support,
        ScenarioReach reach
    ) {
        Minecraft minecraft = requireInPlay();
        BlockPos supportPos = pos(support);
        if (!minecraft.level.isLoaded(supportPos) || minecraft.level.getBlockState(supportPos).isAir()) {
            return null;
        }
        Vec3 eye = minecraft.player.getEyePosition();
        for (Direction direction : HORIZONTAL_DIRECTIONS) {
            BlockPos target = supportPos.relative(direction);
            if (!isEmptyLoaded(target) || isPlayerSpace(target) || !isFluidNeighbourhoodEmpty(target)) {
                continue;
            }
            double distance = eye.distanceToSqr(Vec3.atLowerCornerWithOffset(
                supportPos,
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
            return new ScenarioBlockPair(
                new ScenarioBlockTarget(
                    support.x(),
                    support.y(),
                    support.z(),
                    direction.getName(),
                    "horizontal-attachment-support",
                    blockIdAt(support)
                ),
                new ScenarioBlockTarget(
                    target.getX(),
                    target.getY(),
                    target.getZ(),
                    direction.getOpposite().getName(),
                    "horizontal-attachment-target",
                    blockIdAt(target)
                )
            );
        }
        return null;
    }

    private static ScenarioBlockPair findTillableSoilOnClientThread(ScenarioReach reach) {
        Minecraft minecraft = requireInPlay();
        BlockPos origin = minecraft.player.blockPosition();
        Vec3 eye = minecraft.player.getEyePosition();
        for (int radius = 1; radius <= NATURAL_BREAKABLE_SCAN_RADIUS; radius++) {
            for (int dx = -radius; dx <= radius; dx++) {
                for (int dz = -radius; dz <= radius; dz++) {
                    if (Math.max(Math.abs(dx), Math.abs(dz)) != radius) {
                        continue;
                    }
                    for (int dy = -3; dy <= 1; dy++) {
                        BlockPos clicked = origin.offset(dx, dy, dz);
                        if (!isSolidLoaded(clicked)) {
                            continue;
                        }
                        String blockId = blockIdAt(clicked);
                        if (!isTillableSoilBlockId(blockId)) {
                            continue;
                        }
                        BlockPos target = clicked.above();
                        if (!isEmptyLoaded(target)
                            || isPlayerSpace(target)
                            || !isFluidNeighbourhoodEmpty(target)) {
                            continue;
                        }
                        double distance = eye.distanceToSqr(Vec3.atLowerCornerWithOffset(
                            clicked,
                            0.5,
                            1.0,
                            0.5
                        ));
                        if (reach == ScenarioReach.WITHIN_SURVIVAL_REACH
                            && distance > SURVIVAL_REACH_SQUARED) {
                            continue;
                        }
                        if (reach == ScenarioReach.OUTSIDE_SURVIVAL_REACH
                            && distance <= FAR_REACH_SQUARED) {
                            continue;
                        }
                        return new ScenarioBlockPair(
                            new ScenarioBlockTarget(
                                clicked.getX(),
                                clicked.getY(),
                                clicked.getZ(),
                                "up",
                                reach.label() + "-tillable-soil",
                                blockId
                            ),
                            new ScenarioBlockTarget(
                                target.getX(),
                                target.getY(),
                                target.getZ(),
                                "down",
                                reach.label() + "-crop-target",
                                blockIdAt(target)
                            )
                        );
                    }
                }
            }
        }
        return null;
    }

    private static ScenarioBlockTarget findBreakableBlockOnClientThread(List<String> blockIds, ScenarioReach reach) {
        Minecraft minecraft = requireInPlay();
        BlockPos origin = minecraft.player.blockPosition();
        Vec3 eye = minecraft.player.getEyePosition();
        for (int radius = 1; radius <= NATURAL_BREAKABLE_SCAN_RADIUS; radius++) {
            for (int dx = -radius; dx <= radius; dx++) {
                for (int dz = -radius; dz <= radius; dz++) {
                    if (Math.max(Math.abs(dx), Math.abs(dz)) != radius) {
                        continue;
                    }
                    for (int dy : NATURAL_BREAKABLE_SCAN_VERTICAL_OFFSETS) {
                        if (reach == ScenarioReach.OUTSIDE_SURVIVAL_REACH && !isApproachableVerticalOffset(dy)) {
                            continue;
                        }
                        BlockPos target = origin.offset(dx, dy, dz);
                        ScenarioBlockTarget block = breakableBlockTarget(target, eye, reach, blockIds);
                        if (block != null) {
                            return block;
                        }
                    }
                }
            }
        }
        return null;
    }

    private static int[] naturalBreakableScanVerticalOffsets() {
        int[] offsets = new int[NATURAL_BREAKABLE_SCAN_DOWN + NATURAL_BREAKABLE_SCAN_UP + 1];
        int index = 0;
        offsets[index++] = 0;
        int max = Math.max(NATURAL_BREAKABLE_SCAN_DOWN, NATURAL_BREAKABLE_SCAN_UP);
        for (int step = 1; step <= max; step++) {
            if (step <= NATURAL_BREAKABLE_SCAN_UP) {
                offsets[index++] = step;
            }
            if (step <= NATURAL_BREAKABLE_SCAN_DOWN) {
                offsets[index++] = -step;
            }
        }
        return offsets;
    }

    private static boolean isApproachableVerticalOffset(int dy) {
        return dy >= -APPROACHABLE_SCAN_DOWN && dy <= APPROACHABLE_SCAN_UP;
    }

    private static boolean clearFoliageObstacleTowardOnClientThread(ScenarioBlockTarget target) {
        Minecraft minecraft = requireInPlay();
        Vec3 eye = minecraft.player.getEyePosition();
        Vec3 center = Vec3.atLowerCornerWithOffset(pos(target), 0.5, 0.5, 0.5);
        Vec3 delta = center.subtract(eye);
        int steps = Math.max(1, (int) Math.ceil(delta.length() * 4.0));
        BlockPos targetPos = pos(target);
        for (int step = 1; step < steps; step++) {
            double scale = (double) step / (double) steps;
            Vec3 sample = eye.add(delta.scale(scale));
            BlockPos obstacle = BlockPos.containing(sample);
            if (clearFoliageBlockOnClientThread(minecraft, obstacle, targetPos)) {
                return true;
            }
        }
        return clearNearbyFoliageTowardOnClientThread(minecraft, targetPos, center);
    }

    private static boolean clearNearbyFoliageTowardOnClientThread(
        Minecraft minecraft,
        BlockPos targetPos,
        Vec3 targetCenter
    ) {
        BlockPos origin = minecraft.player.blockPosition();
        double currentDistance = minecraft.player.position().distanceToSqr(targetCenter);
        for (int radius = 1; radius <= 2; radius++) {
            for (int dx = -radius; dx <= radius; dx++) {
                for (int dz = -radius; dz <= radius; dz++) {
                    if (Math.max(Math.abs(dx), Math.abs(dz)) != radius) {
                        continue;
                    }
                    for (int dy = 0; dy <= 2; dy++) {
                        BlockPos obstacle = origin.offset(dx, dy, dz);
                        Vec3 obstacleCenter = Vec3.atLowerCornerWithOffset(obstacle, 0.5, 0.5, 0.5);
                        if (obstacleCenter.distanceToSqr(targetCenter) >= currentDistance) {
                            continue;
                        }
                        if (clearFoliageBlockOnClientThread(minecraft, obstacle, targetPos)) {
                            return true;
                        }
                    }
                }
            }
        }
        return false;
    }

    private static boolean clearFoliageBlockOnClientThread(
        Minecraft minecraft,
        BlockPos obstacle,
        BlockPos targetPos
    ) {
        if (obstacle.equals(targetPos) || !minecraft.level.isLoaded(obstacle)) {
            return false;
        }
        BlockState state = minecraft.level.getBlockState(obstacle);
        if (state.isAir()) {
            return false;
        }
        String blockId = BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString();
        if (!blockId.endsWith("_leaves")) {
            return false;
        }
        minecraft.gameMode.startDestroyBlock(obstacle, Direction.UP);
        minecraft.gameMode.continueDestroyBlock(obstacle, Direction.UP);
        minecraft.player.swing(InteractionHand.MAIN_HAND);
        return true;
    }

    private static ScenarioBlockTarget breakableBlockTarget(
        BlockPos target,
        Vec3 eye,
        ScenarioReach reach,
        List<String> blockIds
    ) {
        if (!isNonAirLoaded(target)) {
            return null;
        }
        String blockId = blockIdAt(target);
        if (!blockIds.contains(blockId)) {
            return null;
        }
        Direction closestDirection = null;
        double closestDistance = Double.POSITIVE_INFINITY;
        for (Direction direction : BREAK_DIRECTIONS) {
            if (!isBreakFaceAccessible(target, direction)) {
                continue;
            }
            double distance = eye.distanceToSqr(Vec3.atLowerCornerWithOffset(
                target,
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
            if (distance < closestDistance) {
                closestDirection = direction;
                closestDistance = distance;
            }
        }
        if (closestDirection == null) {
            return null;
        }
        return new ScenarioBlockTarget(
            target.getX(),
            target.getY(),
            target.getZ(),
            closestDirection.getName(),
            reach.label() + "-breakable",
            blockId
        );
    }

    private static Direction closestAccessibleFace(BlockPos target, Vec3 eye) {
        Direction closestDirection = null;
        double closestDistance = Double.POSITIVE_INFINITY;
        for (Direction direction : BREAK_DIRECTIONS) {
            if (!isBreakFaceAccessible(target, direction)) {
                continue;
            }
            double distance = eye.distanceToSqr(Vec3.atLowerCornerWithOffset(
                target,
                cursorX(direction),
                cursorY(direction),
                cursorZ(direction)
            ));
            if (distance < closestDistance) {
                closestDirection = direction;
                closestDistance = distance;
            }
        }
        return closestDirection;
    }

    private static boolean isBreakFaceAccessible(BlockPos target, Direction direction) {
        Minecraft minecraft = requireInPlay();
        BlockPos neighbour = target.relative(direction);
        if (!minecraft.level.isLoaded(neighbour)) {
            return false;
        }
        BlockState state = minecraft.level.getBlockState(neighbour);
        return state.isAir() || !state.blocksMotion();
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
        boolean requireDryTarget,
        boolean requirePlayerClearance,
        Direction[] directions
    ) {
        if (!isSolidLoaded(clicked)) {
            return null;
        }
        String clickedBlockId = blockIdAt(clicked);
        if (!dropsAsDirt(clickedBlockId)) {
            return null;
        }
        for (Direction direction : directions) {
            BlockPos target = clicked.relative(direction);
            if (!isEmptyLoaded(target) || isPlayerSpace(target)) {
                continue;
            }
            if (requireDryTarget && !isFluidNeighbourhoodEmpty(target)) {
                continue;
            }
            if (
                requirePlayerClearance
                    && !hasFullBlockPlacementClearance(target)
            ) {
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

    private static boolean hasFullBlockPlacementClearance(BlockPos target) {
        Minecraft minecraft = requireInPlay();
        BlockState state = Blocks.CRAFTING_TABLE.defaultBlockState();
        return BlockPlacementClearance.allowsFullBlockPlacement(
            state.canSurvive(minecraft.level, target),
            minecraft.level.isUnobstructed(
                state,
                target,
                CollisionContext.placementContext(minecraft.player)
            )
        );
    }

    private static boolean isPlayerPassable(BlockPos pos) {
        Minecraft minecraft = requireInPlay();
        if (!minecraft.level.isLoaded(pos)) {
            return false;
        }
        BlockState state = minecraft.level.getBlockState(pos);
        return (state.isAir() || !state.blocksMotion()) && minecraft.level.getFluidState(pos).isEmpty();
    }

    private static List<int[]> safeHungerDrainWaypointsOnClientThread() {
        Minecraft minecraft = requireInPlay();
        BlockPos origin = minecraft.player.blockPosition();
        List<int[]> waypoints = new ArrayList<>();
        if (!safeHungerDrainColumn(minecraft, origin)) {
            return waypoints;
        }
        for (Direction direction : HORIZONTAL_DIRECTIONS) {
            BlockPos endpoint = safeHungerDrainEndpoint(minecraft, origin, direction);
            if (endpoint != null) {
                waypoints.add(new int[] {endpoint.getX(), endpoint.getZ()});
                waypoints.add(new int[] {origin.getX(), origin.getZ()});
            }
        }
        return waypoints;
    }

    private static BlockPos safeHungerDrainEndpoint(
        Minecraft minecraft,
        BlockPos origin,
        Direction direction
    ) {
        BlockPos previous = origin;
        int[] verticalOffsets = {0, 1, -1};
        for (int distance = 1; distance <= 6; distance++) {
            BlockPos horizontal = origin.relative(direction, distance);
            BlockPos next = null;
            for (int verticalOffset : verticalOffsets) {
                BlockPos candidate = new BlockPos(
                    horizontal.getX(),
                    previous.getY() + verticalOffset,
                    horizontal.getZ()
                );
                if (safeHungerDrainColumn(minecraft, candidate)) {
                    next = candidate;
                    break;
                }
            }
            if (next == null) {
                return null;
            }
            previous = next;
        }
        return previous;
    }

    private static boolean safeHungerDrainColumn(Minecraft minecraft, BlockPos feet) {
        BlockPos head = feet.above();
        BlockPos support = feet.below();
        if (!minecraft.level.isLoaded(feet)
            || !minecraft.level.isLoaded(head)
            || !minecraft.level.isLoaded(support)) {
            return false;
        }
        BlockState feetState = minecraft.level.getBlockState(feet);
        BlockState headState = minecraft.level.getBlockState(head);
        BlockState supportState = minecraft.level.getBlockState(support);
        return (feetState.isAir() || !feetState.blocksMotion())
            && minecraft.level.getFluidState(feet).isEmpty()
            && !isDamagingHungerDrainBlock(feetState)
            && (headState.isAir() || !headState.blocksMotion())
            && minecraft.level.getFluidState(head).isEmpty()
            && !isDamagingHungerDrainBlock(headState)
            && !supportState.isAir()
            && supportState.blocksMotion()
            && minecraft.level.getFluidState(support).isEmpty()
            && !isDamagingHungerDrainBlock(supportState);
    }

    private static boolean isDamagingHungerDrainBlock(BlockState state) {
        return state.is(Blocks.CACTUS)
            || state.is(Blocks.CAMPFIRE)
            || state.is(Blocks.FIRE)
            || state.is(Blocks.MAGMA_BLOCK)
            || state.is(Blocks.POWDER_SNOW)
            || state.is(Blocks.SOUL_CAMPFIRE)
            || state.is(Blocks.SOUL_FIRE)
            || state.is(Blocks.SWEET_BERRY_BUSH)
            || state.is(Blocks.WITHER_ROSE);
    }

    private static boolean dropsAsDirt(String blockId) {
        return switch (blockId) {
            case "minecraft:dirt", "minecraft:coarse_dirt", "minecraft:grass_block", "minecraft:podzol" -> true;
            default -> false;
        };
    }

    private static boolean isTillableSoilBlockId(String blockId) {
        return switch (blockId) {
            case "minecraft:dirt", "minecraft:grass_block", "minecraft:dirt_path" -> true;
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

    private static boolean isNonAirLoaded(BlockPos pos) {
        Minecraft minecraft = requireInPlay();
        return minecraft.level.isLoaded(pos) && !minecraft.level.getBlockState(pos).isAir();
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
        var playerBox = minecraft.player.getBoundingBox();
        return BlockPlacementClearance.intersects(
            playerBox.minX,
            playerBox.minY,
            playerBox.minZ,
            playerBox.maxX,
            playerBox.maxY,
            playerBox.maxZ,
            pos.getX(),
            pos.getY(),
            pos.getZ()
        );
    }

    private static boolean itemDropVisibleOnClientThread(String itemId, BlockPos near) {
        return itemDropPositionOnClientThread(itemId, near) != null;
    }

    private static List<ScenarioItemDropIdentity> itemDropIdentitiesOnClientThread(String itemId) {
        Minecraft minecraft = requireInPlay();
        List<ScenarioItemDropIdentity> identities = new ArrayList<>();
        for (Entity entity : minecraft.level.entitiesForRendering()) {
            if (
                entity instanceof ItemEntity itemEntity
                    && !entity.isRemoved()
                    && itemStackMatches(itemEntity.getItem(), itemId, 1)
            ) {
                identities.add(new ScenarioItemDropIdentity(entity.getId(), entity.getUUID()));
            }
        }
        return List.copyOf(identities);
    }

    private static ScenarioItemDropIdentity newItemDropIdentityOnClientThread(
        String itemId,
        List<ScenarioItemDropIdentity> excludedIdentities
    ) {
        for (ScenarioItemDropIdentity identity : itemDropIdentitiesOnClientThread(itemId)) {
            if (!excludedIdentities.contains(identity)) {
                return identity;
            }
        }
        return null;
    }

    private static Vec3 itemDropPositionOnClientThread(String itemId, BlockPos near) {
        Minecraft minecraft = requireInPlay();
        Vec3 center = Vec3.atLowerCornerWithOffset(near, 0.5, 0.5, 0.5);
        Vec3 nearest = null;
        double nearestDistance = Double.MAX_VALUE;
        for (Entity entity : minecraft.level.entitiesForRendering()) {
            if (entity instanceof ItemEntity itemEntity && !entity.isRemoved()) {
                ItemStack stack = itemEntity.getItem();
                double distance = entity.distanceToSqr(center);
                if (
                    !stack.isEmpty()
                        && Objects.equals(BuiltInRegistries.ITEM.getKey(stack.getItem()).toString(), itemId)
                        && distance <= 16.0
                ) {
                    if (distance < nearestDistance) {
                        nearestDistance = distance;
                        nearest = entity.position();
                    }
                }
            }
        }
        return nearest;
    }

    private static Vec3 itemDropPositionOnClientThread(String itemId, ScenarioItemDropIdentity identity) {
        Minecraft minecraft = requireInPlay();
        Entity entity = minecraft.level.getEntity(identity.entityId());
        if (
            !(entity instanceof ItemEntity itemEntity)
                || entity.isRemoved()
                || !entity.getUUID().equals(identity.uuid())
        ) {
            return null;
        }
        return itemStackMatches(itemEntity.getItem(), itemId, 1)
            ? entity.position()
            : null;
    }

    private static ScenarioEntityObservation visibleEntityOnClientThread(
        List<String> entityTypeIds,
        ScenarioReach reach
    ) {
        Minecraft minecraft = requireInPlay();
        Vec3 playerPosition = minecraft.player.position();
        ScenarioEntityObservation nearest = null;
        double nearestDistance = Double.MAX_VALUE;
        for (Entity entity : minecraft.level.entitiesForRendering()) {
            if (entity.isRemoved() || entity == minecraft.player) {
                continue;
            }
            String observedTypeId = BuiltInRegistries.ENTITY_TYPE.getKey(entity.getType()).toString();
            if (!entityTypeIds.contains(observedTypeId)) {
                continue;
            }
            double distance = entity.distanceToSqr(playerPosition);
            if (!reach.includes(distance)) {
                continue;
            }
            if (distance < nearestDistance) {
                nearestDistance = distance;
                nearest = entityObservation(entity, playerPosition, distance);
            }
        }
        return nearest;
    }

    private static ScenarioEntityObservation visibleSheepWithWoolOnClientThread(
        String woolItemId,
        ScenarioReach reach
    ) {
        Minecraft minecraft = requireInPlay();
        Vec3 playerPosition = minecraft.player.position();
        ScenarioEntityObservation nearest = null;
        double nearestDistance = Double.MAX_VALUE;
        for (Entity entity : minecraft.level.entitiesForRendering()) {
            if (!(entity instanceof Sheep sheep) || entity.isRemoved() || sheep.isSheared()) {
                continue;
            }
            if (!Objects.equals(sheepWoolItemId(sheep.getColor()), woolItemId)) {
                continue;
            }
            double distance = entity.distanceToSqr(playerPosition);
            if (!reach.includes(distance)) {
                continue;
            }
            if (distance < nearestDistance) {
                nearestDistance = distance;
                nearest = entityObservation(entity, playerPosition, distance);
            }
        }
        return nearest;
    }

    private static ScenarioPlayerObservation visiblePlayerOnClientThread(String playerName) {
        Minecraft minecraft = requireInPlay();
        Vec3 viewerPosition = minecraft.player.position();
        for (Entity entity : minecraft.level.entitiesForRendering()) {
            if (
                entity.isRemoved()
                    || entity == minecraft.player
                    || !(entity instanceof Player player)
                    || !playerName.equals(player.getPlainTextName())
            ) {
                continue;
            }
            return playerObservation(entity, playerName, viewerPosition);
        }
        return null;
    }

    private static Entity entityByIdOnClientThread(int entityId) {
        Minecraft minecraft = requireInPlay();
        for (Entity entity : minecraft.level.entitiesForRendering()) {
            if (!entity.isRemoved() && entity.getId() == entityId) {
                return entity;
            }
        }
        return null;
    }

    static InteractionHand interactionHand(String hand) {
        return switch (hand) {
            case "main_hand" -> InteractionHand.MAIN_HAND;
            case "off_hand" -> InteractionHand.OFF_HAND;
            default -> throw new IllegalArgumentException("interaction hand must be main_hand or off_hand");
        };
    }

    static EntityInteractionDispatch.Outcome interactionOutcome(InteractionResult result) {
        if (result instanceof InteractionResult.Success) {
            return new EntityInteractionDispatch.Outcome("success", true);
        }
        if (result == InteractionResult.FAIL) {
            return new EntityInteractionDispatch.Outcome("fail", false);
        }
        if (result == InteractionResult.PASS) {
            return new EntityInteractionDispatch.Outcome("pass", false);
        }
        if (result == InteractionResult.TRY_WITH_EMPTY_HAND) {
            return new EntityInteractionDispatch.Outcome("try_with_empty_hand", false);
        }
        throw new IllegalStateException("unknown vanilla interaction result: " + result.getClass().getName());
    }

    private static final class MinecraftEntityInteractionAccess
        implements EntityInteractionDispatch.Access<ClientLevel, Entity, EntityHitResult> {
        @Override
        public ClientLevel currentLevel() {
            return Minecraft.getInstance().level;
        }

        @Override
        public Entity entityById(ClientLevel level, int entityId) {
            Entity entity = level.getEntity(entityId);
            return entity == null || entity.isRemoved() ? null : entity;
        }

        @Override
        public ScenarioEntityIdentity identity(Entity entity) {
            return new ScenarioEntityIdentity(
                entity.getId(),
                entity.getUUID(),
                BuiltInRegistries.ENTITY_TYPE.getKey(entity.getType()).toString()
            );
        }

        @Override
        public EntityHitResult currentEntityHit() {
            return Minecraft.getInstance().hitResult instanceof EntityHitResult hit ? hit : null;
        }

        @Override
        public Entity hitEntity(EntityHitResult hit) {
            return hit.getEntity();
        }

        @Override
        public boolean isWithinReach(Entity entity) {
            Minecraft minecraft = requireInPlay();
            return minecraft.player.isWithinEntityInteractionRange(entity, 0.0);
        }

        @Override
        public double hitX(EntityHitResult hit) {
            return hit.getLocation().x;
        }

        @Override
        public double hitY(EntityHitResult hit) {
            return hit.getLocation().y;
        }

        @Override
        public double hitZ(EntityHitResult hit) {
            return hit.getLocation().z;
        }

        @Override
        public EntityInteractionDispatch.Outcome interact(
            Entity entity,
            EntityHitResult hit,
            String hand
        ) {
            Minecraft minecraft = requireInPlay();
            return interactionOutcome(minecraft.gameMode.interact(
                minecraft.player,
                entity,
                hit,
                interactionHand(hand)
            ));
        }
    }

    private static EntityMotionSample entityMotionSampleOnClientThread(ScenarioEntityIdentity identity) {
        Entity entity = entityByIdOnClientThread(identity.entityId());
        if (
            entity == null
                || !identity.matches(
                    entity.getId(),
                    entity.getUUID(),
                    BuiltInRegistries.ENTITY_TYPE.getKey(entity.getType()).toString()
                )
        ) {
            return null;
        }
        return entityMotionSample(entity);
    }

    private static EntityMotionSample entityMotionSample(Entity entity) {
        Vec3 velocity = entity.getDeltaMovement();
        double horizontalSpeed = Math.hypot(velocity.x, velocity.z);
        double yawDelta = Double.NaN;
        if (horizontalSpeed > 0.01) {
            double movementYaw = Math.toDegrees(Math.atan2(-velocity.x, velocity.z));
            yawDelta = wrappedYawDelta(entity.getYRot(), movementYaw);
        }
        return new EntityMotionSample(
            BuiltInRegistries.ENTITY_TYPE.getKey(entity.getType()).toString(),
            entity.getX(),
            entity.getY(),
            entity.getZ(),
            horizontalSpeed,
            yawDelta
        );
    }

    private static double wrappedYawDelta(double first, double second) {
        double delta = (first - second) % 360.0;
        if (delta > 180.0) {
            delta -= 360.0;
        } else if (delta < -180.0) {
            delta += 360.0;
        }
        return Math.abs(delta);
    }

    private static ScenarioEntityObservation entityObservation(Entity entity, Vec3 playerPosition, double distance) {
        return new ScenarioEntityObservation(
            BuiltInRegistries.ENTITY_TYPE.getKey(entity.getType()).toString(),
            entity.getId(),
            entity.getUUID(),
            entity.getX(),
            entity.getY(),
            entity.getZ(),
            distance == Double.MAX_VALUE ? entity.distanceToSqr(playerPosition) : distance,
            entity instanceof Sheep sheep && !sheep.isSheared() ? sheepWoolItemId(sheep.getColor()) : null
        );
    }

    static String sheepWoolItemId(DyeColor color) {
        return SheepWoolColor.itemId(color.name());
    }

    private static ScenarioPlayerObservation playerObservation(
        Entity entity,
        String playerName,
        Vec3 viewerPosition
    ) {
        return new ScenarioPlayerObservation(
            playerName,
            entity.getId(),
            entity.getX(),
            entity.getY(),
            entity.getZ(),
            entity.distanceToSqr(viewerPosition)
        );
    }

    private static boolean chatHistoryContainsOnClientThread(String expectedText) {
        Minecraft minecraft = requireInPlay();
        for (String text : chatMessageTexts(minecraft.gui.getChat())) {
            if (expectedText.equals(text) || text.contains(expectedText)) {
                return true;
            }
        }
        return false;
    }

    private static List<String> chatMessageTexts(ChatComponent chat) {
        try {
            Field allMessagesField = ChatComponent.class.getDeclaredField("allMessages");
            allMessagesField.setAccessible(true);
            Object value = allMessagesField.get(chat);
            if (!(value instanceof List<?> messages)) {
                throw new IllegalStateException("ChatComponent.allMessages is not a List");
            }

            List<String> texts = new ArrayList<>();
            for (Object message : messages) {
                if (message instanceof GuiMessage guiMessage) {
                    texts.add(guiMessage.content().getString());
                }
            }
            return texts;
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("could not read client chat history", error);
        }
    }

    private static double horizontalDistance(ScenarioPlayerObservation a, ScenarioPlayerObservation b) {
        double dx = a.x() - b.x();
        double dz = a.z() - b.z();
        return Math.sqrt(dx * dx + dz * dz);
    }

    private static Vec3 entityLookTarget(Entity entity) {
        return entity.position().add(0.0, entity.getBbHeight() * 0.5, 0.0);
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
                    entity.getUUID(),
                    entity.getX(),
                    entity.getY(),
                    entity.getZ(),
                    entity.distanceToSqr(playerPosition),
                    null
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
        return heldItemFromStack(minecraft.player.getInventory().getSelectedItem());
    }

    private static ScenarioHeldItem heldItemFromStack(ItemStack stack) {
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

    private static int armorMenuSlot(String armorSlot) {
        return switch (armorSlot) {
            case "head" -> 5;
            case "chest" -> 6;
            case "legs" -> 7;
            case "feet" -> 8;
            default -> throw new IllegalArgumentException("unsupported armor slot " + armorSlot);
        };
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

    private static boolean blockPropertyMatches(ScenarioBlockTarget target, String property, String value) {
        Minecraft minecraft = requireInPlay();
        BlockState state = minecraft.level.getBlockState(pos(target));
        return state.getValues().anyMatch(propertyValue ->
            Objects.equals(propertyValue.property().getName(), property)
                && Objects.equals(propertyValue.valueName(), value)
        );
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

    private static BlockHitResult hitResult(ScenarioBlockTarget target, double cursorHeight) {
        Direction direction = direction(target.face());
        return new BlockHitResult(
            Vec3.atLowerCornerWithOffset(pos(target), cursorX(direction), cursorHeight, cursorZ(direction)),
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

    private static long clientTick(Minecraft minecraft) {
        return Integer.toUnsignedLong(minecraft.player.tickCount);
    }

    private static boolean awaitClientStateChange(long observedVersion, long deadlineNanos)
        throws InterruptedException {
        long remainingNanos = deadlineNanos - System.nanoTime();
        return remainingNanos > 0L
            && ClientStateEvents.awaitChange(observedVersion, Duration.ofNanos(remainingNanos));
    }

    private static boolean awaitClientTick(long observedVersion, long deadlineNanos)
        throws InterruptedException {
        long remainingNanos = deadlineNanos - System.nanoTime();
        return remainingNanos > 0L
            && ClientStateEvents.awaitTickChange(
                observedVersion,
                Duration.ofNanos(remainingNanos)
            );
    }

    private record ContainerUpdateCheckpoint(int containerId, int stateId) {
    }

    private record ContainerClickAttempt(
        boolean alreadyMatched,
        ContainerUpdateCheckpoint checkpoint
    ) {
    }

    private record HotbarSelectionAttempt(
        ScenarioHeldItem selectedItem,
        ContainerUpdateCheckpoint checkpoint,
        int targetHotbarSlot
    ) {
    }

    private record HotbarSelectionResponse(
        boolean observed,
        boolean confirmed,
        ScenarioHeldItem selectedItem
    ) {
    }

    private static final class BlockBreakAutomation {
        private final ScenarioBlockTarget target;
        private final Direction face;
        private final CompletableFuture<Void> started = new CompletableFuture<>();
        private boolean startSent;

        private BlockBreakAutomation(ScenarioBlockTarget target, Direction face) {
            this.target = target;
            this.face = face;
        }
    }

    private record BreakSample(
        boolean becameAir,
        boolean sawDrop,
        ScenarioHeldItem selectedItem,
        long tick
    ) {
    }

    private record PickupSample(
        boolean visibleDrop,
        ScenarioHeldItem selectedItem,
        int inventoryCount,
        double x,
        double y,
        double z,
        double dropX,
        double dropY,
        double dropZ,
        double dropDistanceSquared,
        boolean horizontalCollision,
        boolean onGround,
        int detourDirection,
        long tick,
        boolean identityTaken
    ) {
    }

    private static String pickupDetail(PickupSample sample) {
        if (sample == null) {
            return "";
        }
        String drop = sample.visibleDrop()
            ? String.format(Locale.ROOT, "(%.3f,%.3f,%.3f)", sample.dropX(), sample.dropY(), sample.dropZ())
            : "none";
        return String.format(
            Locale.ROOT,
            "player=(%.3f,%.3f,%.3f) drop=%s distance_squared=%.3f horizontal_collision=%s on_ground=%s detour_direction=%d tick=%d",
            sample.x(),
            sample.y(),
            sample.z(),
            drop,
            sample.dropDistanceSquared(),
            sample.horizontalCollision(),
            sample.onGround(),
            sample.detourDirection(),
            sample.tick()
        );
    }

    private record ApproachSample(boolean inReach, int detourDirection) {
    }

    private record HazardStandSample(boolean dead, int detourDirection) {
    }

    private record EntityApproachSample(boolean visible, boolean inReach, int detourDirection) {
    }

    interface EntityWaitSource {
        Object captureLevel() throws Exception;

        EntityStateSnapshot snapshot() throws Exception;

        long stateVersion();

        boolean awaitStateChange(long observedVersion, long deadlineNanos)
            throws InterruptedException;
    }

    record EntityStateSnapshot(Object level, EntityMotionSample motion, boolean present) {
    }

    record EntityMotionSample(
        String entityTypeId,
        double x,
        double y,
        double z,
        double horizontalSpeed,
        double yawDelta
    ) {
    }

    private record EntityAttackSample(
        boolean attackSent,
        boolean removed,
        boolean visibleDrop,
        ScenarioHeldItem selectedItem,
        int inventoryCount,
        double x,
        double y,
        double z,
        long tick
    ) {
    }

    private record EntityRemovalAttackSample(boolean attackSent, boolean removed, long tick) {
    }

    private record FoodUseSample(boolean started, int foodLevel, int itemCount) {
    }

    private record ShieldBlockSample(boolean useStarted, float health, int shieldDamage) {
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
