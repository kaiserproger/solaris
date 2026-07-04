package dev.solaris.agent.bridge;

import com.google.gson.Gson;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;

public final class BridgeCodec {
    private static final Gson GSON = new Gson();

    private BridgeCodec() {
    }

    public static BridgeRequest decodeRequest(String body) {
        JsonObject root = JsonParser.parseString(body).getAsJsonObject();
        JsonObject payload = root.has("payload") && root.get("payload").isJsonObject()
            ? root.getAsJsonObject("payload")
            : new JsonObject();
        return new BridgeRequest(
            root.get("id").getAsLong(),
            root.get("secret").getAsString(),
            root.get("command").getAsString(),
            payload
        );
    }

    public static String encodeResponse(BridgeResponse response) {
        return GSON.toJson(response);
    }
}
