package dev.solaris.agent.bridge;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class BridgeCodecTest {
    @Test
    void decodesRequestWithIdCommandSecretAndPayload() {
        BridgeRequest request = BridgeCodec.decodeRequest("""
            {"id":7,"secret":"run-secret","command":"ping","payload":{"client":"probe"}}
            """);

        assertEquals(7L, request.id());
        assertEquals("run-secret", request.secret());
        assertEquals("ping", request.command());
        assertEquals("probe", request.payload().get("client").getAsString());
    }

    @Test
    void encodesSuccessResponseWithPayload() {
        JsonObject payload = new JsonObject();
        payload.addProperty("bridge_version", "0.1.0");

        String encoded = BridgeCodec.encodeResponse(BridgeResponse.success(8L, payload));

        assertTrue(encoded.contains("\"id\":8"));
        assertTrue(encoded.contains("\"ok\":true"));
        assertTrue(encoded.contains("\"bridge_version\":\"0.1.0\""));
        assertFalse(encoded.contains("\"error\""));
    }

    @Test
    void encodesStructuredErrorResponse() {
        BridgeError error = new BridgeError("unknown-command", "unsupported command: mine");

        String encoded = BridgeCodec.encodeResponse(BridgeResponse.failure(9L, error));

        assertTrue(encoded.contains("\"id\":9"));
        assertTrue(encoded.contains("\"ok\":false"));
        assertTrue(encoded.contains("\"code\":\"unknown-command\""));
        assertTrue(encoded.contains("\"message\":\"unsupported command: mine\""));
    }
}
