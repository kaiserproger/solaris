import com.google.gson.stream.JsonWriter;

import java.io.FileWriter;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;

import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.FarmlandBlock;
import net.minecraft.world.level.block.FenceBlock;
import net.minecraft.world.level.block.SlabBlock;
import net.minecraft.world.level.block.StairBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.VoxelShape;

public final class CollisionShapeExtractor {
    private static final String EXPECTED_VERSION = "26.1.2";
    private static final int UNITS_PER_BLOCK = 16;

    private CollisionShapeExtractor() {}

    private record StateShape(int stateId, int shapeIndex) {}

    public static void main(String[] args) throws IOException {
        if (args.length != 1) {
            System.err.println("usage: java CollisionShapeExtractor <output.json>");
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

        Map<String, Integer> familyCounts = new TreeMap<>();
        Map<String, Integer> shapeIndexes = new LinkedHashMap<>();
        List<List<int[]>> shapes = new ArrayList<>();
        List<StateShape> entries = new ArrayList<>();

        for (Block block : BuiltInRegistries.BLOCK) {
            String family = family(block);
            if (family == null) {
                continue;
            }
            for (BlockState state : block.getStateDefinition().getPossibleStates()) {
                int stateId = Block.BLOCK_STATE_REGISTRY.getId(state);
                VoxelShape shape = state.getCollisionShape(
                    EmptyBlockGetter.INSTANCE,
                    BlockPos.ZERO
                );
                List<int[]> boxes = quantizedBoxes(stateId, shape);
                String key = shapeKey(boxes);
                Integer shapeIndex = shapeIndexes.get(key);
                if (shapeIndex == null) {
                    shapeIndex = shapes.size();
                    shapeIndexes.put(key, shapeIndex);
                    shapes.add(boxes);
                }
                entries.add(new StateShape(stateId, shapeIndex));
                familyCounts.merge(family, 1, Integer::sum);
            }
        }

        entries.sort((left, right) -> Integer.compare(left.stateId(), right.stateId()));
        for (int index = 1; index < entries.size(); index++) {
            if (entries.get(index - 1).stateId() >= entries.get(index).stateId()) {
                throw new IllegalStateException("duplicate or unsorted block state id");
            }
        }
        for (String family : List.of("farmland", "fence", "slab", "stairs")) {
            if (familyCounts.getOrDefault(family, 0) == 0) {
                throw new IllegalStateException("vanilla registry had no " + family + " states");
            }
        }

        try (JsonWriter writer = new JsonWriter(new FileWriter(args[0]))) {
            writer.setIndent("");
            writer.beginObject();
            writer.name("version").value(version);
            writer.name("units_per_block").value(UNITS_PER_BLOCK);
            writer.name("family_state_counts").beginObject();
            for (Map.Entry<String, Integer> count : familyCounts.entrySet()) {
                writer.name(count.getKey()).value(count.getValue());
            }
            writer.endObject();
            writer.name("shapes").beginArray();
            for (List<int[]> shape : shapes) {
                writer.beginArray();
                for (int[] box : shape) {
                    writer.beginArray();
                    for (int coordinate : box) {
                        writer.value(coordinate);
                    }
                    writer.endArray();
                }
                writer.endArray();
            }
            writer.endArray();
            writer.name("entries").beginArray();
            for (StateShape entry : entries) {
                writer.beginArray();
                writer.value(entry.stateId());
                writer.value(entry.shapeIndex());
                writer.endArray();
            }
            writer.endArray();
            writer.endObject();
        }

        System.err.printf(
            "vanilla %s: wrote %d covered states and %d unique shapes %s%n",
            version,
            entries.size(),
            shapes.size(),
            familyCounts
        );
    }

    private static String family(Block block) {
        if (block instanceof FarmlandBlock) return "farmland";
        if (block instanceof FenceBlock) return "fence";
        if (block instanceof SlabBlock) return "slab";
        if (block instanceof StairBlock) return "stairs";
        return null;
    }

    private static List<int[]> quantizedBoxes(int stateId, VoxelShape shape) {
        if (shape.isEmpty()) {
            throw new IllegalStateException("covered state " + stateId + " has empty collision shape");
        }
        List<int[]> boxes = new ArrayList<>();
        for (AABB box : shape.toAabbs()) {
            int[] coordinates = {
                quantize(stateId, box.minX),
                quantize(stateId, box.minY),
                quantize(stateId, box.minZ),
                quantize(stateId, box.maxX),
                quantize(stateId, box.maxY),
                quantize(stateId, box.maxZ),
            };
            if (coordinates[0] >= coordinates[3]
                || coordinates[1] >= coordinates[4]
                || coordinates[2] >= coordinates[5]) {
                throw new IllegalStateException("degenerate collision box for state " + stateId);
            }
            boxes.add(coordinates);
        }
        boxes.sort(Arrays::compare);
        return boxes;
    }

    private static int quantize(int stateId, double coordinate) {
        double scaled = coordinate * UNITS_PER_BLOCK;
        int rounded = (int)Math.round(scaled);
        if (Math.abs(scaled - rounded) > 1.0e-7 || rounded < 0 || rounded > 255) {
            throw new IllegalStateException(
                "state " + stateId + " has unsupported collision coordinate " + coordinate
            );
        }
        return rounded;
    }

    private static String shapeKey(List<int[]> boxes) {
        StringBuilder key = new StringBuilder();
        for (int[] box : boxes) {
            key.append(Arrays.toString(box));
        }
        return key.toString();
    }
}
