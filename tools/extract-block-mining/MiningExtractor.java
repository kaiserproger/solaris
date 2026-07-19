import com.google.gson.stream.JsonWriter;

import java.io.FileWriter;
import java.io.IOException;

import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;

public final class MiningExtractor {
    private MiningExtractor() {}

    public static void main(String[] args) throws IOException {
        if (args.length != 1) {
            System.err.println("usage: java MiningExtractor <output.json>");
            System.exit(2);
        }

        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        int maxId = -1;
        for (Block block : BuiltInRegistries.BLOCK) {
            for (BlockState state : block.getStateDefinition().getPossibleStates()) {
                maxId = Math.max(maxId, Block.BLOCK_STATE_REGISTRY.getId(state));
            }
        }
        if (maxId < 0) {
            throw new IllegalStateException("block registry yielded no states");
        }

        float[] destroySpeed = new float[maxId + 1];
        boolean[] requiresCorrectTool = new boolean[maxId + 1];
        boolean[] present = new boolean[maxId + 1];
        for (Block block : BuiltInRegistries.BLOCK) {
            for (BlockState state : block.getStateDefinition().getPossibleStates()) {
                int id = Block.BLOCK_STATE_REGISTRY.getId(state);
                destroySpeed[id] = state.getDestroySpeed(EmptyBlockGetter.INSTANCE, BlockPos.ZERO);
                requiresCorrectTool[id] = state.requiresCorrectToolForDrops();
                present[id] = true;
            }
        }

        int gaps = 0;
        for (boolean statePresent : present) {
            if (!statePresent) {
                gaps++;
            }
        }

        try (JsonWriter writer = new JsonWriter(new FileWriter(args[0]))) {
            writer.setIndent("");
            writer.beginObject();
            writer.name("version").value(SharedConstants.getCurrentVersion().name());
            writer.name("max_state_id").value(maxId);
            writer.name("entries").beginArray();
            for (int id = 0; id <= maxId; id++) {
                writer.beginArray();
                writer.value(destroySpeed[id]);
                writer.value(requiresCorrectTool[id] ? 1 : 0);
                writer.endArray();
            }
            writer.endArray();
            writer.endObject();
        }

        System.err.printf("wrote %d entries (max_state_id=%d, gaps=%d)%n",
            maxId + 1, maxId, gaps);
    }
}
