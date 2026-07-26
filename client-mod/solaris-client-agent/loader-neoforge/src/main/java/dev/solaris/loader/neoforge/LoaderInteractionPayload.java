package dev.solaris.loader.neoforge;

import dev.solaris.loader.LoaderInteractionAction;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;
import net.minecraft.resources.Identifier;

record LoaderInteractionPayload(byte[] bytes) implements CustomPacketPayload {
    static final Type<LoaderInteractionPayload> TYPE =
            new Type<>(Identifier.parse("solaris:loader/interaction"));
    static final StreamCodec<FriendlyByteBuf, LoaderInteractionPayload> CODEC =
            CustomPacketPayload.codec(
                    LoaderInteractionPayload::write,
                    LoaderInteractionPayload::new);

    LoaderInteractionPayload {
        bytes = bytes.clone();
    }

    private LoaderInteractionPayload(FriendlyByteBuf buffer) {
        this(read(buffer));
    }

    private void write(FriendlyByteBuf buffer) {
        buffer.writeBytes(bytes);
    }

    private static byte[] read(FriendlyByteBuf buffer) {
        int length = buffer.readableBytes();
        if (length > LoaderInteractionAction.MAX_PAYLOAD_BYTES) {
            throw new IllegalArgumentException("Loader interaction payload exceeds limit");
        }
        byte[] bytes = new byte[length];
        buffer.readBytes(bytes);
        return bytes;
    }

    @Override
    public byte[] bytes() {
        return bytes.clone();
    }

    @Override
    public Type<? extends CustomPacketPayload> type() {
        return TYPE;
    }
}
