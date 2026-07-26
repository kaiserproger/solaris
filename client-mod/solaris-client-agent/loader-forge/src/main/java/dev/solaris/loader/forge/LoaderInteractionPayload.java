package dev.solaris.loader.forge;

import dev.solaris.loader.LoaderInteractionAction;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.network.protocol.common.custom.CustomPacketPayload;
import net.minecraft.resources.Identifier;

record LoaderInteractionPayload(byte[] bytes) implements CustomPacketPayload {
    static final Type<LoaderInteractionPayload> TYPE =
            new Type<>(Identifier.parse("solaris:loader/interaction"));
    static final StreamCodec<RegistryFriendlyByteBuf, LoaderInteractionPayload> CODEC =
            CustomPacketPayload.codec(
                    LoaderInteractionPayload::write,
                    LoaderInteractionPayload::new);

    LoaderInteractionPayload {
        bytes = bytes.clone();
    }

    private LoaderInteractionPayload(RegistryFriendlyByteBuf buffer) {
        this(read(buffer));
    }

    private void write(RegistryFriendlyByteBuf buffer) {
        buffer.writeBytes(bytes);
    }

    private static byte[] read(RegistryFriendlyByteBuf buffer) {
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
