package dev.solaris.agent.mixin;

import dev.solaris.agent.javaagent.ClientStateEvents;
import dev.solaris.agent.javaagent.ScenarioItemDropIdentity;
import net.minecraft.client.Minecraft;
import net.minecraft.client.multiplayer.ClientPacketListener;
import net.minecraft.network.protocol.game.ClientboundBlockChangedAckPacket;
import net.minecraft.network.protocol.game.ClientboundContainerClosePacket;
import net.minecraft.network.protocol.game.ClientboundContainerSetContentPacket;
import net.minecraft.network.protocol.game.ClientboundContainerSetDataPacket;
import net.minecraft.network.protocol.game.ClientboundContainerSetSlotPacket;
import net.minecraft.network.protocol.game.ClientboundOpenScreenPacket;
import net.minecraft.network.protocol.game.ClientboundSetHealthPacket;
import net.minecraft.network.protocol.game.ClientboundSetTimePacket;
import net.minecraft.network.protocol.game.ClientboundTakeItemEntityPacket;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.item.ItemEntity;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

@Mixin(ClientPacketListener.class)
abstract class ClientPacketListenerMixin {
    @Inject(method = "handleBlockChangedAck", at = @At("RETURN"))
    private void solaris$publishBlockChangeAck(
        ClientboundBlockChangedAckPacket packet,
        CallbackInfo callbackInfo
    ) {
        ClientStateEvents.publishBlockChangeAck();
    }

    @Inject(method = "handleTakeItemEntity", at = @At("HEAD"))
    private void solaris$publishTakenItemIdentity(
        ClientboundTakeItemEntityPacket packet,
        CallbackInfo callbackInfo
    ) {
        Minecraft minecraft = Minecraft.getInstance();
        if (
            minecraft.level == null
                || minecraft.player == null
                || packet.getPlayerId() != minecraft.player.getId()
        ) {
            return;
        }
        Entity entity = minecraft.level.getEntity(packet.getItemId());
        if (entity instanceof ItemEntity && !entity.isRemoved()) {
            ClientStateEvents.publishItemTaken(
                new ScenarioItemDropIdentity(entity.getId(), entity.getUUID()),
                packet.getPlayerId()
            );
        }
    }

    @Inject(method = "handleSetTime", at = @At("RETURN"))
    private void solaris$publishServerTime(ClientboundSetTimePacket packet, CallbackInfo callbackInfo) {
        ClientStateEvents.publishServerTime(packet.gameTime());
    }

    @Inject(method = "handleSetHealth", at = @At("RETURN"))
    private void solaris$publishHealth(ClientboundSetHealthPacket packet, CallbackInfo callbackInfo) {
        ClientStateEvents.publishHealth();
    }

    @Inject(method = "handleOpenScreen", at = @At("RETURN"))
    private void solaris$publishOpenScreen(ClientboundOpenScreenPacket packet, CallbackInfo callbackInfo) {
        ClientStateEvents.publishContainerPacket();
    }

    @Inject(method = "handleContainerSetSlot", at = @At("RETURN"))
    private void solaris$publishContainerSlot(
        ClientboundContainerSetSlotPacket packet,
        CallbackInfo callbackInfo
    ) {
        ClientStateEvents.publishContainerPacket();
    }

    @Inject(method = "handleContainerContent", at = @At("RETURN"))
    private void solaris$publishContainerContent(
        ClientboundContainerSetContentPacket packet,
        CallbackInfo callbackInfo
    ) {
        ClientStateEvents.publishContainerPacket();
    }

    @Inject(method = "handleContainerSetData", at = @At("RETURN"))
    private void solaris$publishContainerData(
        ClientboundContainerSetDataPacket packet,
        CallbackInfo callbackInfo
    ) {
        ClientStateEvents.publishContainerPacket();
    }

    @Inject(method = "handleContainerClose", at = @At("RETURN"))
    private void solaris$publishContainerClose(
        ClientboundContainerClosePacket packet,
        CallbackInfo callbackInfo
    ) {
        ClientStateEvents.publishContainerPacket();
    }
}
