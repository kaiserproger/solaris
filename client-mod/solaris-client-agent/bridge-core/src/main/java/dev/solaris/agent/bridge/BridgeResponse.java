package dev.solaris.agent.bridge;

import com.google.gson.JsonObject;

public record BridgeResponse(long id, boolean ok, JsonObject payload, BridgeError error) {
    public static BridgeResponse success(long id, JsonObject payload) {
        return new BridgeResponse(id, true, payload, null);
    }

    public static BridgeResponse failure(long id, BridgeError error) {
        return new BridgeResponse(id, false, new JsonObject(), error);
    }
}
