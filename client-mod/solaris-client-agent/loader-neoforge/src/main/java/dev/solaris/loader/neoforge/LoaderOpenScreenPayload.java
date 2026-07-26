package dev.solaris.loader.neoforge;

import dev.solaris.loader.LoaderScreenOpen;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;
import net.minecraft.resources.Identifier;

record LoaderOpenScreenPayload(byte[] bytes) implements CustomPacketPayload {
    static final Type<LoaderOpenScreenPayload> TYPE =
            new Type<>(Identifier.parse("solaris:loader/open_screen"));
    static final StreamCodec<FriendlyByteBuf, LoaderOpenScreenPayload> CODEC =
            CustomPacketPayload.codec(
                    LoaderOpenScreenPayload::write,
                    LoaderOpenScreenPayload::new);

    LoaderOpenScreenPayload {
        bytes = bytes.clone();
    }

    private LoaderOpenScreenPayload(FriendlyByteBuf buffer) {
        this(read(buffer));
    }

    private void write(FriendlyByteBuf buffer) {
        buffer.writeBytes(bytes);
    }

    private static byte[] read(FriendlyByteBuf buffer) {
        int length = buffer.readableBytes();
        if (length > LoaderScreenOpen.MAX_PAYLOAD_BYTES) {
            throw new IllegalArgumentException("Loader open-screen payload exceeds limit");
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
