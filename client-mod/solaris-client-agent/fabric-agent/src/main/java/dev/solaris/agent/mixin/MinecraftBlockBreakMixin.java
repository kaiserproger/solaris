package dev.solaris.agent.mixin;

import dev.solaris.agent.javaagent.MinecraftScenarioClient;
import net.minecraft.client.Minecraft;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

@Mixin(Minecraft.class)
abstract class MinecraftBlockBreakMixin {
    @Inject(method = "continueAttack", at = @At("HEAD"), cancellable = true)
    private void solaris$letTheAutomationOwnBlockBreaking(
        boolean active,
        CallbackInfo callbackInfo
    ) {
        if (MinecraftScenarioClient.hasActiveBlockBreak()) {
            callbackInfo.cancel();
        }
    }
}
