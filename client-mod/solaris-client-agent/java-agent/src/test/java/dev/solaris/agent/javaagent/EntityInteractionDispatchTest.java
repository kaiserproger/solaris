package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientTaskExecutor;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.InteractionResult;
import org.junit.jupiter.api.Test;

import java.util.UUID;
import java.util.concurrent.Callable;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class EntityInteractionDispatchTest {
    private static final ScenarioEntityIdentity TARGET = new ScenarioEntityIdentity(
        42,
        UUID.fromString("01234567-89ab-cdef-0123-456789abcdef"),
        "minecraft:cow"
    );

    @Test
    void rejectsEntityIdReuseBeforeDispatch() {
        Object level = new Object();
        FakeAccess access = new FakeAccess(level, entity(new ScenarioEntityIdentity(
            42,
            UUID.fromString("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
            "minecraft:pig"
        )));

        IllegalStateException error = assertThrows(
            IllegalStateException.class,
            () -> EntityInteractionDispatch.execute(level, request("main_hand"), access)
        );

        assertTrue(error.getMessage().contains("before dispatch"));
        assertEquals(0, access.interactCalls);
    }

    @Test
    void rejectsEntityIdReuseAfterDispatch() {
        Object level = new Object();
        FakeAccess access = readyAccess(level);
        access.afterInteract = () -> access.entity = entity(new ScenarioEntityIdentity(
            42,
            UUID.fromString("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
            "minecraft:pig"
        ));

        IllegalStateException error = assertThrows(
            IllegalStateException.class,
            () -> EntityInteractionDispatch.execute(level, request("main_hand"), access)
        );

        assertTrue(error.getMessage().contains("after dispatch"));
        assertEquals(1, access.interactCalls);
    }

    @Test
    void queuedDispatchRejectsAWorldTransitionEvenWhenTheIdentityRepeats() {
        Object originalLevel = new Object();
        Object replacementLevel = new Object();
        FakeAccess access = readyAccess(originalLevel);
        ClientTaskExecutor executor = new ClientTaskExecutor() {
            @Override
            public <T> T callOnClientThread(Callable<T> callable) throws Exception {
                access.level = replacementLevel;
                access.entity = entity(TARGET);
                access.hit = new Hit(access.entity, 8.25, 65.5, -3.75);
                return callable.call();
            }
        };

        IllegalStateException error = assertThrows(
            IllegalStateException.class,
            () -> EntityInteractionDispatch.queue(executor, request("main_hand"), access)
        );

        assertTrue(error.getMessage().contains("world changed before dispatch"));
        assertEquals(0, access.interactCalls);
    }

    @Test
    void rejectsAWorldTransitionAfterDispatchEvenWhenTheIdentityRepeats() {
        Object originalLevel = new Object();
        Object replacementLevel = new Object();
        FakeAccess access = readyAccess(originalLevel);
        access.afterInteract = () -> {
            access.level = replacementLevel;
            access.entity = entity(TARGET);
        };

        IllegalStateException error = assertThrows(
            IllegalStateException.class,
            () -> EntityInteractionDispatch.execute(originalLevel, request("main_hand"), access)
        );

        assertTrue(error.getMessage().contains("world changed after dispatch"));
        assertEquals(1, access.interactCalls);
    }

    @Test
    void rejectsOccludedNonTargetAndDistantCrosshairTargets() {
        Object level = new Object();

        FakeAccess occluded = readyAccess(level);
        occluded.hit = null;
        assertTrue(assertThrows(
            IllegalStateException.class,
            () -> EntityInteractionDispatch.execute(level, request("main_hand"), occluded)
        ).getMessage().contains("crosshair"));

        FakeAccess nonTarget = readyAccess(level);
        nonTarget.hit = new Hit(entity(new ScenarioEntityIdentity(
            7,
            UUID.fromString("11111111-2222-3333-4444-555555555555"),
            "minecraft:pig"
        )), 1.0, 2.0, 3.0);
        assertTrue(assertThrows(
            IllegalStateException.class,
            () -> EntityInteractionDispatch.execute(level, request("main_hand"), nonTarget)
        ).getMessage().contains("different entity"));

        FakeAccess distant = readyAccess(level);
        distant.withinReach = false;
        assertTrue(assertThrows(
            IllegalStateException.class,
            () -> EntityInteractionDispatch.execute(level, request("main_hand"), distant)
        ).getMessage().contains("out of reach"));
    }

    @Test
    void dispatchesTheActualHitForBothHandsAndReturnsObservedResults() throws Exception {
        Object level = new Object();
        FakeAccess mainHand = readyAccess(level);
        mainHand.outcome = new EntityInteractionDispatch.Outcome("success", true);

        ScenarioEntityInteractionResult success = EntityInteractionDispatch.queue(
            Callable::call,
            request("main_hand"),
            mainHand
        );

        assertEquals("main_hand", mainHand.hand);
        assertSame(mainHand.hit, mainHand.dispatchedHit);
        assertEquals("success", success.result());
        assertTrue(success.consumesAction());
        assertEquals(8.25, success.hitX());
        assertEquals(65.5, success.hitY());
        assertEquals(-3.75, success.hitZ());

        FakeAccess offHand = readyAccess(level);
        offHand.outcome = new EntityInteractionDispatch.Outcome("pass", false);
        ScenarioEntityInteractionResult pass = EntityInteractionDispatch.execute(
            level,
            request("off_hand"),
            offHand
        );

        assertEquals("off_hand", offHand.hand);
        assertSame(offHand.hit, offHand.dispatchedHit);
        assertEquals("pass", pass.result());
        assertEquals(false, pass.consumesAction());
    }

    @Test
    void mapsSupportedHandsAndObservedVanillaResults() {
        assertEquals(InteractionHand.MAIN_HAND, MinecraftScenarioClient.interactionHand("main_hand"));
        assertEquals(InteractionHand.OFF_HAND, MinecraftScenarioClient.interactionHand("off_hand"));
        assertEquals(
            new EntityInteractionDispatch.Outcome("success", true),
            MinecraftScenarioClient.interactionOutcome(InteractionResult.SUCCESS)
        );
        assertEquals(
            new EntityInteractionDispatch.Outcome("pass", false),
            MinecraftScenarioClient.interactionOutcome(InteractionResult.PASS)
        );
        assertEquals(
            new EntityInteractionDispatch.Outcome("fail", false),
            MinecraftScenarioClient.interactionOutcome(InteractionResult.FAIL)
        );
        assertEquals(
            new EntityInteractionDispatch.Outcome("try_with_empty_hand", false),
            MinecraftScenarioClient.interactionOutcome(InteractionResult.TRY_WITH_EMPTY_HAND)
        );
    }

    private static ScenarioEntityInteraction request(String hand) {
        return new ScenarioEntityInteraction(TARGET, hand);
    }

    private static EntityState entity(ScenarioEntityIdentity identity) {
        return new EntityState(identity);
    }

    private static FakeAccess readyAccess(Object level) {
        EntityState entity = entity(TARGET);
        FakeAccess access = new FakeAccess(level, entity);
        access.hit = new Hit(entity, 8.25, 65.5, -3.75);
        return access;
    }

    private record EntityState(ScenarioEntityIdentity identity) {
    }

    private record Hit(EntityState entity, double x, double y, double z) {
    }

    private static final class FakeAccess
        implements EntityInteractionDispatch.Access<Object, EntityState, Hit> {
        private Object level;
        private EntityState entity;
        private Hit hit;
        private boolean withinReach = true;
        private EntityInteractionDispatch.Outcome outcome =
            new EntityInteractionDispatch.Outcome("pass", false);
        private Runnable afterInteract = () -> { };
        private int interactCalls;
        private String hand;
        private Hit dispatchedHit;

        private FakeAccess(Object level, EntityState entity) {
            this.level = level;
            this.entity = entity;
        }

        @Override
        public Object currentLevel() {
            return level;
        }

        @Override
        public EntityState entityById(Object expectedLevel, int entityId) {
            return expectedLevel == level && entity.identity().entityId() == entityId ? entity : null;
        }

        @Override
        public ScenarioEntityIdentity identity(EntityState observed) {
            return observed.identity();
        }

        @Override
        public Hit currentEntityHit() {
            return hit;
        }

        @Override
        public EntityState hitEntity(Hit observedHit) {
            return observedHit.entity();
        }

        @Override
        public boolean isWithinReach(EntityState target) {
            return withinReach;
        }

        @Override
        public double hitX(Hit observedHit) {
            return observedHit.x();
        }

        @Override
        public double hitY(Hit observedHit) {
            return observedHit.y();
        }

        @Override
        public double hitZ(Hit observedHit) {
            return observedHit.z();
        }

        @Override
        public EntityInteractionDispatch.Outcome interact(EntityState target, Hit observedHit, String hand) {
            interactCalls += 1;
            this.hand = hand;
            dispatchedHit = observedHit;
            afterInteract.run();
            return outcome;
        }
    }
}
