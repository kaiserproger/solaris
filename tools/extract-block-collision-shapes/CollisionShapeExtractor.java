import java.io.BufferedOutputStream;
import java.io.DataOutputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.VoxelShape;

public final class CollisionShapeExtractor {
    private static final String EXPECTED_VERSION = "26.1.2";
    private static final int UNITS_PER_BLOCK = 4096;
    private static final byte[] MAGIC = {'S', 'O', 'L', 'C', 'O', 'L', 'L', '1'};
    private static final int MISSING_SHAPE = 0xffff;

    private CollisionShapeExtractor() {}

    private record StateShape(int stateId, int shapeIndex, long fingerprint) {}

    public static void main(String[] args) throws IOException {
        if (args.length != 1) {
            System.err.println("usage: java CollisionShapeExtractor <output.bin>");
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

        Map<String, Integer> shapeIndexes = new LinkedHashMap<>();
        List<List<int[]>> shapes = new ArrayList<>();
        List<StateShape> entries = new ArrayList<>();

        for (Block block : BuiltInRegistries.BLOCK) {
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
                entries.add(new StateShape(stateId, shapeIndex, stateFingerprint(block, state)));
            }
        }

        entries.sort((left, right) -> Integer.compare(left.stateId(), right.stateId()));
        for (int index = 1; index < entries.size(); index++) {
            if (entries.get(index - 1).stateId() >= entries.get(index).stateId()) {
                throw new IllegalStateException("duplicate or unsorted block state id");
            }
        }
        if (shapes.size() >= MISSING_SHAPE) {
            throw new IllegalStateException("too many unique collision shapes: " + shapes.size());
        }

        int stateCount = entries.get(entries.size() - 1).stateId() + 1;
        int boxCount = shapes.stream().mapToInt(List::size).sum();
        int maxBoxY = shapes.stream()
            .flatMap(List::stream)
            .mapToInt(box -> box[4])
            .max()
            .orElse(0);
        int[] stateShapes = new int[stateCount];
        Arrays.fill(stateShapes, MISSING_SHAPE);
        long[] fingerprints = new long[stateCount];
        for (StateShape entry : entries) {
            stateShapes[entry.stateId()] = entry.shapeIndex();
            fingerprints[entry.stateId()] = entry.fingerprint();
        }

        try (DataOutputStream writer = new DataOutputStream(
            new BufferedOutputStream(new FileOutputStream(args[0]))
        )) {
            writer.write(MAGIC);
            writer.writeShort(UNITS_PER_BLOCK);
            writer.writeShort(maxBoxY);
            writer.writeInt(stateCount);
            writer.writeInt(shapes.size());
            writer.writeInt(boxCount);
            writer.writeInt(1);
            writer.writeInt(0);
            for (int shape : stateShapes) writer.writeShort(shape);
            for (long fingerprint : fingerprints) writer.writeLong(fingerprint);
            int offset = 0;
            writer.writeInt(offset);
            for (List<int[]> shape : shapes) {
                offset += shape.size();
                writer.writeInt(offset);
            }
            for (List<int[]> shape : shapes) {
                for (int[] box : shape) {
                    for (int coordinate : box) writer.writeShort(coordinate);
                }
            }
        }

        System.err.printf(
            "vanilla %s: wrote %d states, %d unique shapes and %d boxes%n",
            version,
            entries.size(),
            shapes.size(),
            boxCount
        );
    }

    private static List<int[]> quantizedBoxes(int stateId, VoxelShape shape) {
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
        if (Math.abs(scaled - rounded) > 1.0e-7
            || rounded < Short.MIN_VALUE
            || rounded > Short.MAX_VALUE) {
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

    private static long stateFingerprint(Block block, BlockState state) {
        List<Property<?>> properties = new ArrayList<>(state.getProperties());
        properties.sort((left, right) -> left.getName().compareTo(right.getName()));
        long hash = fnv1a(0xcbf29ce484222325L, BuiltInRegistries.BLOCK.getKey(block).toString());
        for (Property<?> property : properties) {
            hash = fnv1a(hash, "\0" + property.getName() + "=" + propertyValueName(state, property));
        }
        return hash;
    }

    private static <T extends Comparable<T>> String propertyValueName(
        BlockState state,
        Property<T> property
    ) {
        return property.getName(state.getValue(property));
    }

    private static long fnv1a(long hash, String value) {
        for (byte octet : value.getBytes(java.nio.charset.StandardCharsets.UTF_8)) {
            hash ^= octet & 0xffL;
            hash *= 0x100000001b3L;
        }
        return hash;
    }
}
