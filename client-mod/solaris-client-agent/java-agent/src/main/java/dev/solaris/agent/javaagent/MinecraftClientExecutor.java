package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientTaskExecutor;
import net.minecraft.client.Minecraft;

import java.util.concurrent.Callable;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

public final class MinecraftClientExecutor implements ClientTaskExecutor {
    private static final long CLIENT_THREAD_TIMEOUT_SECONDS = 10L;

    @Override
    public <T> T callOnClientThread(Callable<T> callable) throws Exception {
        Minecraft minecraft = Minecraft.getInstance();
        if (minecraft.isSameThread()) {
            return callable.call();
        }
        CompletableFuture<T> future = new CompletableFuture<>();
        minecraft.execute(() -> {
            if (future.isDone()) {
                return;
            }
            try {
                future.complete(callable.call());
            } catch (Throwable error) {
                future.completeExceptionally(error);
            }
        });
        try {
            return future.get(CLIENT_THREAD_TIMEOUT_SECONDS, TimeUnit.SECONDS);
        } catch (InterruptedException error) {
            future.cancel(false);
            Thread.currentThread().interrupt();
            throw error;
        } catch (TimeoutException error) {
            future.cancel(false);
            throw new IllegalStateException("Minecraft client thread did not respond within 10 seconds", error);
        } catch (ExecutionException error) {
            Throwable cause = error.getCause();
            if (cause instanceof Exception exception) {
                throw exception;
            }
            if (cause instanceof Error fatal) {
                throw fatal;
            }
            throw new IllegalStateException("Minecraft client-thread call failed", cause);
        }
    }
}
