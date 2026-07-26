package dev.solaris.agent.mixin;

import com.mojang.logging.LogUtils;
import net.minecraft.client.multiplayer.ClientCommonPacketListenerImpl;
import net.minecraft.network.protocol.Packet;
import net.minecraft.network.protocol.game.ServerboundPlayerActionPacket;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;
import org.slf4j.Logger;

@Mixin(ClientCommonPacketListenerImpl.class)
abstract class ClientCommonPacketListenerMixin {
    private static final Logger SOLARIS_LOGGER = LogUtils.getLogger();
    private static final boolean SOLARIS_TRACE_BLOCK_BREAK = Boolean.getBoolean(
        "solaris.traceBlockBreak"
    );

    @Inject(method = "send", at = @At("HEAD"))
    private void solaris$traceBlockBreakAction(Packet<?> packet, CallbackInfo callbackInfo) {
        if (!(packet instanceof ServerboundPlayerActionPacket action)) {
            return;
        }
        if (
            action.getAction() != ServerboundPlayerActionPacket.Action.START_DESTROY_BLOCK
                && action.getAction() != ServerboundPlayerActionPacket.Action.ABORT_DESTROY_BLOCK
                && action.getAction() != ServerboundPlayerActionPacket.Action.STOP_DESTROY_BLOCK
        ) {
            return;
        }
        if (SOLARIS_TRACE_BLOCK_BREAK) {
            SOLARIS_LOGGER.info(
                "[solaris-block-break] send action={} pos={} direction={} sequence={}",
                action.getAction(),
                action.getPos(),
                action.getDirection(),
                action.getSequence()
            );
        }
    }
}
