package dev.solaris.agent.javaagent;

import java.time.Duration;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.concurrent.locks.Condition;
import java.util.concurrent.locks.ReentrantLock;

public final class ClientStateEvents {
    private static final int MAX_ITEM_TAKEN_EVENTS = 64;
    private static final ReentrantLock LOCK = new ReentrantLock();
    private static final Condition STATE_CHANGED = LOCK.newCondition();
    private static final Condition TICK_CHANGED = LOCK.newCondition();
    private static final Condition SERVER_TIME_CHANGED = LOCK.newCondition();
    private static final Condition BLOCK_CHANGE_ACKED = LOCK.newCondition();
    private static final Map<ScenarioItemDropIdentity, Integer> ITEM_TAKEN_BY = new LinkedHashMap<>();
    private static long stateVersion;
    private static long tickVersion;
    private static long serverTimeVersion;
    private static long blockChangeAckVersion;
    private static long serverGameTime = Long.MIN_VALUE;

    private ClientStateEvents() {
    }

    public static long version() {
        LOCK.lock();
        try {
            return stateVersion;
        } finally {
            LOCK.unlock();
        }
    }

    public static long tickVersion() {
        LOCK.lock();
        try {
            return tickVersion;
        } finally {
            LOCK.unlock();
        }
    }

    public static long serverTimeVersion() {
        LOCK.lock();
        try {
            return serverTimeVersion;
        } finally {
            LOCK.unlock();
        }
    }

    public static long blockChangeAckVersion() {
        LOCK.lock();
        try {
            return blockChangeAckVersion;
        } finally {
            LOCK.unlock();
        }
    }

    public static long serverGameTime() {
        LOCK.lock();
        try {
            return serverGameTime;
        } finally {
            LOCK.unlock();
        }
    }

    public static void publishState() {
        LOCK.lock();
        try {
            stateVersion += 1;
            STATE_CHANGED.signalAll();
        } finally {
            LOCK.unlock();
        }
    }

    public static void publishTick() {
        LOCK.lock();
        try {
            tickVersion += 1;
            TICK_CHANGED.signalAll();
        } finally {
            LOCK.unlock();
        }
    }

    public static void publishServerTime(long gameTime) {
        LOCK.lock();
        try {
            serverGameTime = gameTime;
            serverTimeVersion += 1;
            SERVER_TIME_CHANGED.signalAll();
        } finally {
            LOCK.unlock();
        }
    }

    public static void publishBlockChangeAck() {
        LOCK.lock();
        try {
            blockChangeAckVersion += 1;
            BLOCK_CHANGE_ACKED.signalAll();
        } finally {
            LOCK.unlock();
        }
    }

    public static void publishItemTaken(ScenarioItemDropIdentity identity, int playerEntityId) {
        LOCK.lock();
        try {
            ITEM_TAKEN_BY.put(identity, playerEntityId);
            while (ITEM_TAKEN_BY.size() > MAX_ITEM_TAKEN_EVENTS) {
                ITEM_TAKEN_BY.remove(ITEM_TAKEN_BY.keySet().iterator().next());
            }
            stateVersion += 1;
            STATE_CHANGED.signalAll();
        } finally {
            LOCK.unlock();
        }
    }

    public static void clearItemTakenEvents() {
        LOCK.lock();
        try {
            ITEM_TAKEN_BY.clear();
        } finally {
            LOCK.unlock();
        }
    }

    public static boolean consumeItemTakenBy(ScenarioItemDropIdentity identity, int playerEntityId) {
        LOCK.lock();
        try {
            Integer collectorEntityId = ITEM_TAKEN_BY.get(identity);
            if (collectorEntityId == null || collectorEntityId != playerEntityId) {
                return false;
            }
            ITEM_TAKEN_BY.remove(identity);
            return true;
        } finally {
            LOCK.unlock();
        }
    }

    public static boolean awaitChange(long observedVersion, Duration timeout)
        throws InterruptedException {
        long remainingNanos = timeout.toNanos();
        LOCK.lockInterruptibly();
        try {
            while (stateVersion == observedVersion && remainingNanos > 0L) {
                remainingNanos = STATE_CHANGED.awaitNanos(remainingNanos);
            }
            return stateVersion != observedVersion;
        } finally {
            LOCK.unlock();
        }
    }

    public static boolean awaitTickChange(long observedVersion, Duration timeout)
        throws InterruptedException {
        long remainingNanos = timeout.toNanos();
        LOCK.lockInterruptibly();
        try {
            while (tickVersion == observedVersion && remainingNanos > 0L) {
                remainingNanos = TICK_CHANGED.awaitNanos(remainingNanos);
            }
            return tickVersion != observedVersion;
        } finally {
            LOCK.unlock();
        }
    }

    public static boolean awaitServerTimeChange(long observedVersion, Duration timeout)
        throws InterruptedException {
        long remainingNanos = timeout.toNanos();
        LOCK.lockInterruptibly();
        try {
            while (serverTimeVersion == observedVersion && remainingNanos > 0L) {
                remainingNanos = SERVER_TIME_CHANGED.awaitNanos(remainingNanos);
            }
            return serverTimeVersion != observedVersion;
        } finally {
            LOCK.unlock();
        }
    }

    public static boolean awaitBlockChangeAck(long observedVersion, Duration timeout)
        throws InterruptedException {
        long remainingNanos = timeout.toNanos();
        LOCK.lockInterruptibly();
        try {
            while (blockChangeAckVersion == observedVersion && remainingNanos > 0L) {
                remainingNanos = BLOCK_CHANGE_ACKED.awaitNanos(remainingNanos);
            }
            return blockChangeAckVersion != observedVersion;
        } finally {
            LOCK.unlock();
        }
    }
}
