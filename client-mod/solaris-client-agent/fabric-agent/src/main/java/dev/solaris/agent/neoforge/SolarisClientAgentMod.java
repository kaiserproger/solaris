package dev.solaris.agent.neoforge;

import dev.solaris.agent.javaagent.SolarisClientAgent;
import dev.solaris.agent.javaagent.ClientStateEvents;
import dev.solaris.agent.javaagent.MinecraftScenarioClient;
import net.minecraft.client.Minecraft;
import net.neoforged.fml.common.Mod;
import net.neoforged.neoforge.client.event.ClientPlayerNetworkEvent;
import net.neoforged.neoforge.client.event.ClientTickEvent;
import net.neoforged.neoforge.common.NeoForge;

@Mod("solaris_client_agent")
public final class SolarisClientAgentMod {
    private static boolean started;

    public SolarisClientAgentMod() {
        NeoForge.EVENT_BUS.addListener(SolarisClientAgentMod::onClientTickPre);
        NeoForge.EVENT_BUS.addListener(SolarisClientAgentMod::onClientTickPost);
        NeoForge.EVENT_BUS.addListener(SolarisClientAgentMod::onClientLogin);
        NeoForge.EVENT_BUS.addListener(SolarisClientAgentMod::onClientLogout);
        NeoForge.EVENT_BUS.addListener(SolarisClientAgentMod::onClientClone);
    }

    private static void onClientTickPre(ClientTickEvent.Pre event) {
        MinecraftScenarioClient.runPreTickActions();
    }

    private static void onClientTickPost(ClientTickEvent.Post event) {
        ClientStateEvents.publishTick();
        if (!started && Minecraft.getInstance() != null) {
            started = SolarisClientAgent.startFromRuntime();
        }
    }

    private static void onClientLogin(ClientPlayerNetworkEvent.LoggingIn event) {
        ClientStateEvents.publishState();
    }

    private static void onClientLogout(ClientPlayerNetworkEvent.LoggingOut event) {
        ClientStateEvents.publishState();
    }

    private static void onClientClone(ClientPlayerNetworkEvent.Clone event) {
        ClientStateEvents.publishState();
    }
}
