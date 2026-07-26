package dev.solaris.loader.forge;

import dev.solaris.loader.LoaderHandshake;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;
import net.minecraft.resources.Identifier;

public record LoaderRequestPayload(byte[] bytes) implements CustomPacketPayload {
    public static final Type<LoaderRequestPayload> TYPE =
            new Type<>(Identifier.fromNamespaceAndPath("solaris", "loader/request"));
    public static final StreamCodec<FriendlyByteBuf, LoaderRequestPayload> CODEC =
            StreamCodec.of(
                    (buffer, payload) -> buffer.writeBytes(payload.bytes),
                    buffer -> new LoaderRequestPayload(readBytes(buffer)));

    public LoaderRequestPayload {
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
            throw new IllegalArgumentException("invalid Solaris Loader request length " + length);
        }
        byte[] bytes = new byte[length];
        buffer.readBytes(bytes);
        return bytes;
    }
}
