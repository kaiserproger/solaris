package dev.solaris.loader.forge;

import dev.solaris.loader.LoaderScreenOpen;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;
import net.minecraft.resources.Identifier;

record LoaderOpenScreenPayload(byte[] bytes) implements CustomPacketPayload {
    static final Type<LoaderOpenScreenPayload> TYPE =
            new Type<>(Identifier.parse("solaris:loader/open_screen"));
    static final StreamCodec<RegistryFriendlyByteBuf, LoaderOpenScreenPayload> CODEC =
            CustomPacketPayload.codec(
                    LoaderOpenScreenPayload::write,
                    LoaderOpenScreenPayload::new);

    LoaderOpenScreenPayload {
        bytes = bytes.clone();
    }

    private LoaderOpenScreenPayload(RegistryFriendlyByteBuf buffer) {
        this(read(buffer));
    }

    private void write(RegistryFriendlyByteBuf buffer) {
        buffer.writeBytes(bytes);
    }

    private static byte[] read(RegistryFriendlyByteBuf buffer) {
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
