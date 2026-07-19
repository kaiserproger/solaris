import com.google.gson.stream.JsonWriter;

import java.io.FileWriter;
import java.io.IOException;

import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;

public final class ExplosionExtractor {
    private static final String EXPECTED_VERSION = "26.1.2";

    private ExplosionExtractor() {}

    public static void main(String[] args) throws IOException {
        if (args.length != 1) {
            System.err.println("usage: java ExplosionExtractor <output.json>");
            System.exit(2);
        }

        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        String version = SharedConstants.getCurrentVersion().name();
        if (!EXPECTED_VERSION.equals(version)) {
            throw new IllegalStateException(
                "expected vanilla " + EXPECTED_VERSION + ", got " + version
            );
        }

        int maxId = -1;
        for (Block block : BuiltInRegistries.BLOCK) {
            for (BlockState state : block.getStateDefinition().getPossibleStates()) {
                maxId = Math.max(maxId, Block.BLOCK_STATE_REGISTRY.getId(state));
            }
        }
        if (maxId < 0) {
            throw new IllegalStateException("block registry yielded no states");
        }

        float[] resistance = new float[maxId + 1];
        boolean[] present = new boolean[maxId + 1];
        for (Block block : BuiltInRegistries.BLOCK) {
            float blockResistance = block.getExplosionResistance();
            if (!Float.isFinite(blockResistance) || blockResistance < 0.0F) {
                throw new IllegalStateException(
                    "block " + BuiltInRegistries.BLOCK.getKey(block)
                        + " has invalid explosion resistance " + blockResistance
                );
            }
            for (BlockState state : block.getStateDefinition().getPossibleStates()) {
                int id = Block.BLOCK_STATE_REGISTRY.getId(state);
                if (id < 0) {
                    throw new IllegalStateException("block state was absent from the global registry");
                }
                if (present[id]) {
                    throw new IllegalStateException("duplicate global block state ID " + id);
                }
                float fluidResistance = state.getFluidState().getExplosionResistance();
                if (!Float.isFinite(fluidResistance) || fluidResistance < 0.0F) {
                    throw new IllegalStateException(
                        "block state " + id + " has invalid fluid explosion resistance "
                            + fluidResistance
                    );
                }
                resistance[id] = Math.max(blockResistance, fluidResistance);
                present[id] = true;
            }
        }

        int gaps = 0;
        int firstGap = -1;
        for (int id = 0; id < present.length; id++) {
            if (!present[id]) {
                gaps++;
                if (firstGap < 0) {
                    firstGap = id;
                }
            }
        }
        if (gaps != 0) {
            throw new IllegalStateException(
                "global block state registry has " + gaps + " gap(s); first missing ID " + firstGap
            );
        }

        try (JsonWriter writer = new JsonWriter(new FileWriter(args[0]))) {
            writer.setIndent("");
            writer.beginObject();
            writer.name("version").value(version);
            writer.name("max_state_id").value(maxId);
            writer.name("entries").beginArray();
            for (float value : resistance) {
                writer.value(value);
            }
            writer.endArray();
            writer.endObject();
        }

        System.err.printf(
            "vanilla %s: wrote %d entries (max_state_id=%d, gaps=%d)%n",
            version,
            maxId + 1,
            maxId,
            gaps
        );
    }
}
