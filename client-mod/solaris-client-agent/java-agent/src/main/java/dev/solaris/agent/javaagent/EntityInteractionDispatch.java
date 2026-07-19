package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientTaskExecutor;

import java.util.Objects;

final class EntityInteractionDispatch {
    private EntityInteractionDispatch() {
    }

    static <L, E, H> ScenarioEntityInteractionResult queue(
        ClientTaskExecutor executor,
        ScenarioEntityInteraction interaction,
        Access<L, E, H> access
    ) throws Exception {
        L expectedLevel = access.currentLevel();
        return executor.callOnClientThread(() -> execute(expectedLevel, interaction, access));
    }

    static <L, E, H> ScenarioEntityInteractionResult execute(
        L expectedLevel,
        ScenarioEntityInteraction interaction,
        Access<L, E, H> access
    ) throws Exception {
        requireSameLevel(expectedLevel, access.currentLevel(), interaction.identity(), "before dispatch");
        E target = access.entityById(expectedLevel, interaction.identity().entityId());
        requireIdentity(target, interaction.identity(), access, "before dispatch");

        H hit = access.currentEntityHit();
        if (hit == null) {
            throw new IllegalStateException(
                "crosshair does not currently target the fenced entity " + describe(interaction.identity())
            );
        }
        if (access.hitEntity(hit) != target) {
            throw new IllegalStateException(
                "crosshair targets a different entity than the fenced target "
                    + describe(interaction.identity())
            );
        }
        if (!access.isWithinReach(target)) {
            throw new IllegalStateException(
                "fenced entity is out of reach for the current game mode "
                    + describe(interaction.identity())
            );
        }

        double hitX = access.hitX(hit);
        double hitY = access.hitY(hit);
        double hitZ = access.hitZ(hit);
        Outcome outcome = access.interact(target, hit, interaction.hand());

        requireSameLevel(expectedLevel, access.currentLevel(), interaction.identity(), "after dispatch");
        E observedAfter = access.entityById(expectedLevel, interaction.identity().entityId());
        requireIdentity(observedAfter, interaction.identity(), access, "after dispatch");
        if (observedAfter != target) {
            throw staleTarget(interaction.identity(), "after dispatch");
        }
        return new ScenarioEntityInteractionResult(
            outcome.result(),
            outcome.consumesAction(),
            hitX,
            hitY,
            hitZ
        );
    }

    private static <L> void requireSameLevel(
        L expectedLevel,
        L observedLevel,
        ScenarioEntityIdentity identity,
        String phase
    ) {
        if (expectedLevel == null || observedLevel != expectedLevel) {
            throw new IllegalStateException(
                "entity interaction world changed " + phase + ": " + describe(identity)
            );
        }
    }

    private static <L, E, H> void requireIdentity(
        E entity,
        ScenarioEntityIdentity identity,
        Access<L, E, H> access,
        String phase
    ) {
        if (entity == null || !identity.equals(access.identity(entity))) {
            throw staleTarget(identity, phase);
        }
    }

    private static IllegalStateException staleTarget(ScenarioEntityIdentity identity, String phase) {
        return new IllegalStateException(
            "stale entity interaction target " + phase + ": " + describe(identity)
        );
    }

    private static String describe(ScenarioEntityIdentity identity) {
        return "entity_id=" + identity.entityId()
            + " entity_uuid=" + identity.entityUuid()
            + " entity_type=" + identity.entityType();
    }

    interface Access<L, E, H> {
        L currentLevel();

        E entityById(L level, int entityId);

        ScenarioEntityIdentity identity(E entity);

        H currentEntityHit();

        E hitEntity(H hit);

        boolean isWithinReach(E entity);

        double hitX(H hit);

        double hitY(H hit);

        double hitZ(H hit);

        Outcome interact(E entity, H hit, String hand) throws Exception;
    }

    record Outcome(String result, boolean consumesAction) {
        Outcome {
            Objects.requireNonNull(result, "interaction result");
            if (result.isBlank()) {
                throw new IllegalArgumentException("interaction result must not be blank");
            }
        }
    }
}
