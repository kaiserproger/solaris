import it.unimi.dsi.fastutil.objects.Object2ObjectOpenHashMap;

public final class FastutilAttributeOrderOracle {
    private static final double BASE = 10_000_000_000_000_000.0;

    private static final class IdentifierKey {
        private final String namespace;
        private final String path;

        private IdentifierKey(int id) {
            this.namespace = "test";
            this.path = "modifier_" + Integer.toHexString(id);
        }

        @Override
        public int hashCode() {
            return 31 * namespace.hashCode() + path.hashCode();
        }

        @Override
        public boolean equals(Object value) {
            return value instanceof IdentifierKey other
                && namespace.equals(other.namespace)
                && path.equals(other.path);
        }

        @Override
        public String toString() {
            return namespace + ":" + path;
        }
    }

    private static IdentifierKey key(int id) {
        return new IdentifierKey(id);
    }

    private static long valueBits(Object2ObjectOpenHashMap<IdentifierKey, Double> modifiers) {
        double value = BASE;
        for (double amount : modifiers.values()) {
            value += amount;
        }
        return Double.doubleToLongBits(value);
    }

    private static void expect(
        String fixture,
        Object2ObjectOpenHashMap<IdentifierKey, Double> modifiers,
        long expectedBits
    ) {
        long actualBits = valueBits(modifiers);
        if (actualBits != expectedBits) {
            throw new AssertionError(
                fixture + ": expected 0x" + Long.toHexString(expectedBits)
                    + ", got 0x" + Long.toHexString(actualBits)
            );
        }
        System.out.printf("%s=0x%016x%n", fixture, actualBits);
    }

    public static void main(String[] arguments) {
        var collision = new Object2ObjectOpenHashMap<IdentifierKey, Double>();
        collision.put(key(2), 1.0);
        collision.put(key(3), -BASE);
        expect("collision-forward", collision, 0x3ff0000000000000L);

        var collisionReverse = new Object2ObjectOpenHashMap<IdentifierKey, Double>();
        collisionReverse.put(key(3), -BASE);
        collisionReverse.put(key(2), 1.0);
        expect("collision-reverse", collisionReverse, 0x0000000000000000L);

        var removal = new Object2ObjectOpenHashMap<IdentifierKey, Double>();
        removal.put(key(21), 0.0);
        removal.put(key(39), -BASE);
        removal.put(key(2), 1.0);
        expect("wrapped-removal-before", removal, 0x0000000000000000L);
        removal.remove(key(21));
        expect("wrapped-removal-after", removal, 0x3ff0000000000000L);

        var resize = new Object2ObjectOpenHashMap<IdentifierKey, Double>();
        for (int id = 0; id < 24; id++) {
            resize.put(key(id), id == 2 ? 1.0 : id == 3 ? -BASE : 0.0);
        }
        expect("resize-before", resize, 0x3ff0000000000000L);
        resize.put(key(24), 0.0);
        expect("resize-after", resize, 0x0000000000000000L);
        int[] removed = {0, 1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15};
        for (int id : removed) {
            resize.remove(key(id));
        }
        expect("shrink-after-removals", resize, 0x3ff0000000000000L);

        var copied = new Object2ObjectOpenHashMap<IdentifierKey, Double>();
        copied.putAll(collision);
        expect("put-all-source", collision, 0x3ff0000000000000L);
        expect("put-all-target", copied, 0x0000000000000000L);
    }
}
