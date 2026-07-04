package dev.solaris.agent.client;

import java.util.concurrent.Callable;

public interface ClientTaskExecutor {
    <T> T callOnClientThread(Callable<T> callable) throws Exception;
}
