package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientTaskExecutor;
import net.minecraft.client.Minecraft;

import java.util.concurrent.Callable;
import java.util.concurrent.CompletableFuture;

public final class MinecraftClientExecutor implements ClientTaskExecutor {
    @Override
    public <T> T callOnClientThread(Callable<T> callable) throws Exception {
        Minecraft minecraft = Minecraft.getInstance();
        if (minecraft.isSameThread()) {
            return callable.call();
        }
        CompletableFuture<T> future = new CompletableFuture<>();
        minecraft.execute(() -> {
            try {
                future.complete(callable.call());
            } catch (Exception error) {
                future.completeExceptionally(error);
            }
        });
        return future.get();
    }
}
