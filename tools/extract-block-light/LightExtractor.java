// Iterates BuiltInRegistries.BLOCK against the unobfuscated vanilla
// 26.1.x server classes and writes per-block-state light metadata to
// a JSON file. Per ADR 0001 we treat the resulting JSON as data, not
// code: identifiers, field semantics, and the iteration come from
// public Mojang APIs (BlockBehaviour$BlockStateBase.getLightEmission,
// getLightDampening, propagatesSkylightDown), the same posture as
// vanilla's `--reports` data generator.
//
// Output JSON shape:
//   {
//     "version": "<vanilla version id, e.g. 26.1.2>",
//     "max_state_id": <largest BlockState id seen>,
//     "entries": [
//        [emission, dampening, propagates_sky_0_or_1, suffocating_0_or_1],
//        ...    // index = global state-id; states with no Block
//        ...    // entry in the registry get [0, 0, 1] (sentinel).
//     ]
//   }

import com.google.gson.stream.JsonWriter;

import java.io.FileWriter;
import java.io.IOException;
import java.util.Arrays;

import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;

public final class LightExtractor {
    private LightExtractor() {}

    public static void main(String[] args) throws IOException {
        if (args.length != 1) {
            System.err.println("usage: java LightExtractor <output.json>");
            System.exit(2);
        }

        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        int maxId = -1;
        for (Block block : BuiltInRegistries.BLOCK) {
            for (BlockState state : block.getStateDefinition().getPossibleStates()) {
                int id = Block.BLOCK_STATE_REGISTRY.getId(state);
                if (id > maxId) maxId = id;
            }
        }
        if (maxId < 0) {
            System.err.println("error: registry yielded no states");
            System.exit(1);
        }

        int[][] entries = new int[maxId + 1][];
        for (Block block : BuiltInRegistries.BLOCK) {
            for (BlockState state : block.getStateDefinition().getPossibleStates()) {
                int id = Block.BLOCK_STATE_REGISTRY.getId(state);
                int emission = state.getLightEmission();
                int dampening = state.getLightDampening();
                boolean propagates = state.propagatesSkylightDown();
                boolean suffocating = state.isSuffocating(EmptyBlockGetter.INSTANCE, BlockPos.ZERO);
                entries[id] = new int[] {
                    emission,
                    dampening,
                    propagates ? 1 : 0,
                    suffocating ? 1 : 0
                };
            }
        }

        int gaps = 0;
        for (int i = 0; i <= maxId; i++) {
            if (entries[i] == null) {
                gaps++;
                entries[i] = new int[] { 0, 0, 1, 0 };
            }
        }

        try (JsonWriter w = new JsonWriter(new FileWriter(args[0]))) {
            w.setIndent("");
            w.beginObject();
            w.name("version").value(SharedConstants.getCurrentVersion().name());
            w.name("max_state_id").value(maxId);
            w.name("entries").beginArray();
            for (int[] e : entries) {
                w.beginArray();
                w.value(e[0]);
                w.value(e[1]);
                w.value(e[2]);
                w.value(e[3]);
                w.endArray();
            }
            w.endArray();
            w.endObject();
        }

        System.err.printf("wrote %d entries (max_state_id=%d, gaps=%d)%n",
            entries.length, maxId, gaps);
    }
}
