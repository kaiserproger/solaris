package dev.solaris.loader.fabric;

import dev.solaris.loader.LoaderHandshake;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;
import net.minecraft.resources.Identifier;

public record LoaderManifestPayload(byte[] bytes) implements CustomPacketPayload {
    public static final Type<LoaderManifestPayload> TYPE =
            new Type<>(Identifier.fromNamespaceAndPath("solaris", "loader/manifest"));
    public static final StreamCodec<FriendlyByteBuf, LoaderManifestPayload> CODEC =
            StreamCodec.of(
                    (buffer, payload) -> buffer.writeBytes(payload.bytes),
                    buffer -> new LoaderManifestPayload(readBytes(buffer)));

    public LoaderManifestPayload {
        bytes = bytes.clone();
    }

    @Override
    public byte[] bytes() {
        return bytes.clone();
    }

    @Override
    public Type<? extends CustomPacketPayload> type() {
        return TYPE;
    }

    private static byte[] readBytes(FriendlyByteBuf buffer) {
        int length = buffer.readableBytes();
        if (length <= 0 || length > LoaderHandshake.MAX_MANIFEST_BYTES) {
            throw new IllegalArgumentException("invalid Solaris Loader manifest length " + length);
        }
        byte[] bytes = new byte[length];
        buffer.readBytes(bytes);
        return bytes;
    }
}
