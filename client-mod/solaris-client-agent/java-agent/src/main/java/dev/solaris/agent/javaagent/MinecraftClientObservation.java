package dev.solaris.agent.javaagent;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.components.ChatComponent;
import net.minecraft.client.gui.screens.DisconnectedScreen;
import net.minecraft.client.multiplayer.chat.GuiMessage;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.FluidTags;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.inventory.AbstractContainerMenu;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.LightLayer;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.EntityHitResult;
import net.minecraft.world.phys.HitResult;
import net.minecraft.world.phys.Vec3;

import java.lang.reflect.Field;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Locale;

final class MinecraftClientObservation {
    private static final int RECENT_CHAT_LIMIT = 20;

    private MinecraftClientObservation() {
    }

    static JsonObject observe(Minecraft minecraft) {
        JsonObject observation = new JsonObject();
        boolean inPlay = minecraft.player != null && minecraft.level != null;
        observation.addProperty("in_play", inPlay);
        observation.add("screen", screen(minecraft));
        observation.add("recent_chat", recentChat(minecraft));
        if (minecraft.screen instanceof DisconnectedScreen disconnectedScreen) {
            observation.addProperty(
                "disconnect_reason",
                disconnectedScreen.getNarrationMessage().getString()
            );
        }
        if (!inPlay) {
            return observation;
        }

        observation.addProperty("dimension", minecraft.level.dimension().identifier().toString());
        observation.addProperty(
            "water_tag_entries",
            waterTagEntries()
        );
        observation.add("water_tag_fluids", waterTagFluids());
        observation.add("player", player(minecraft));
        observation.add("selected_item", item(minecraft.player.getInventory().getSelectedItem()));
        observation.add("inventory", inventory(minecraft));
        observation.add("container", container(minecraft.player.containerMenu));
        observation.add("target", target(minecraft));

        JsonObject time = new JsonObject();
        time.addProperty("game_time", minecraft.level.getGameTime());
        time.addProperty("clock_time", minecraft.level.getDefaultClockTime());
        time.addProperty("overworld_clock_time", minecraft.level.getOverworldClockTime());
        time.addProperty("day", minecraft.level.getDefaultClockTime() / 24_000L);
        observation.add("time", time);
        return observation;
    }

    private static int waterTagEntries() {
        int count = 0;
        for (var ignored : BuiltInRegistries.FLUID.getTagOrEmpty(FluidTags.WATER)) {
            count++;
        }
        return count;
    }

    private static JsonArray waterTagFluids() {
        JsonArray fluids = new JsonArray();
        for (var fluid : BuiltInRegistries.FLUID.getTagOrEmpty(FluidTags.WATER)) {
            fluids.add(BuiltInRegistries.FLUID.getKey(fluid.value()).toString());
        }
        return fluids;
    }

    static JsonObject readBlock(Minecraft minecraft, BlockPos position) {
        if (!minecraft.level.isLoaded(position)) {
            throw new IllegalStateException("client chunk is not loaded at " + position.toShortString());
        }
        BlockState state = minecraft.level.getBlockState(position);
        FluidState fluid = minecraft.level.getFluidState(position);
        JsonObject block = new JsonObject();
        block.addProperty("x", position.getX());
        block.addProperty("y", position.getY());
        block.addProperty("z", position.getZ());
        block.addProperty("block_id", BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString());
        block.addProperty("is_air", state.isAir());
        block.addProperty("collision_empty", state.getCollisionShape(minecraft.level, position).isEmpty());
        block.addProperty("sky_light", minecraft.level.getBrightness(LightLayer.SKY, position));
        block.addProperty("block_light", minecraft.level.getBrightness(LightLayer.BLOCK, position));

        JsonObject properties = new JsonObject();
        state.getValues().forEach(value ->
            properties.addProperty(value.property().getName(), value.valueName())
        );
        block.add("properties", properties);

        JsonObject fluidJson = new JsonObject();
        fluidJson.addProperty("fluid_id", BuiltInRegistries.FLUID.getKey(fluid.getType()).toString());
        fluidJson.addProperty("empty", fluid.isEmpty());
        fluidJson.addProperty("source", fluid.isSource());
        fluidJson.addProperty("amount", fluid.getAmount());
        fluidJson.addProperty("height", fluid.getHeight(minecraft.level, position));
        fluidJson.addProperty("own_height", fluid.getOwnHeight());
        fluidJson.addProperty("in_water_tag", fluid.is(FluidTags.WATER));
        block.add("fluid", fluidJson);
        return block;
    }

    static JsonObject scanBlocks(
        Minecraft minecraft,
        int minX,
        int minY,
        int minZ,
        int maxX,
        int maxY,
        int maxZ,
        int maxBlocks
    ) {
        int count = boundedVolume(minX, minY, minZ, maxX, maxY, maxZ, maxBlocks);

        JsonArray blocks = new JsonArray();
        for (long y = minY; y <= maxY; y++) {
            for (long z = minZ; z <= maxZ; z++) {
                for (long x = minX; x <= maxX; x++) {
                    blocks.add(readBlock(minecraft, new BlockPos((int) x, (int) y, (int) z)));
                }
            }
        }
        JsonObject result = new JsonObject();
        result.addProperty("count", count);
        result.add("bounds", bounds(minX, minY, minZ, maxX, maxY, maxZ));
        result.add("blocks", blocks);
        return result;
    }

    static JsonObject listEntities(Minecraft minecraft, double radius, int limit) {
        Vec3 viewer = minecraft.player.position();
        double radiusSquared = radius * radius;
        List<VisibleEntity> visible = new ArrayList<>();
        for (Entity entity : minecraft.level.entitiesForRendering()) {
            if (entity.isRemoved()) {
                continue;
            }
            double distanceSquared = entity.distanceToSqr(viewer);
            if (distanceSquared <= radiusSquared) {
                visible.add(new VisibleEntity(entity, distanceSquared));
            }
        }
        visible.sort(Comparator
            .comparingDouble(VisibleEntity::distanceSquared)
            .thenComparingInt(entry -> entry.entity().getId()));

        JsonArray entities = new JsonArray();
        int returned = Math.min(limit, visible.size());
        for (int index = 0; index < returned; index++) {
            VisibleEntity entry = visible.get(index);
            entities.add(entity(entry.entity(), entry.distanceSquared(), minecraft.player));
        }
        JsonObject result = new JsonObject();
        result.addProperty("count", returned);
        result.addProperty("visible_count", visible.size());
        result.addProperty("truncated", visible.size() > returned);
        result.add("entities", entities);
        return result;
    }

    private static JsonObject player(Minecraft minecraft) {
        JsonObject player = entity(minecraft.player, 0.0, minecraft.player);
        player.addProperty("health", minecraft.player.getHealth());
        player.addProperty("max_health", minecraft.player.getMaxHealth());
        player.addProperty("food", minecraft.player.getFoodData().getFoodLevel());
        player.addProperty("saturation", minecraft.player.getFoodData().getSaturationLevel());
        player.addProperty("air", minecraft.player.getAirSupply());
        player.addProperty("max_air", minecraft.player.getMaxAirSupply());
        player.addProperty("experience_level", minecraft.player.experienceLevel);
        player.addProperty("experience_progress", minecraft.player.experienceProgress);
        player.addProperty("total_experience", minecraft.player.totalExperience);
        player.addProperty("selected_hotbar_slot", minecraft.player.getInventory().getSelectedSlot());
        player.addProperty("pose", minecraft.player.getPose().toString().toLowerCase(Locale.ROOT));
        return player;
    }

    private static JsonObject entity(Entity entity, double distanceSquared, Entity localPlayer) {
        ScenarioEntityIdentity identity = new ScenarioEntityIdentity(
            entity.getId(),
            entity.getUUID(),
            BuiltInRegistries.ENTITY_TYPE.getKey(entity.getType()).toString()
        );
        JsonObject value = entityIdentity(identity);
        value.addProperty("name", entity.getName().getString());
        value.addProperty("x", entity.getX());
        value.addProperty("y", entity.getY());
        value.addProperty("z", entity.getZ());
        value.addProperty("yaw", entity.getYRot());
        value.addProperty("pitch", entity.getXRot());
        value.addProperty("distance", Math.sqrt(distanceSquared));
        value.addProperty("local_player", entity == localPlayer);
        value.addProperty("on_ground", entity.onGround());
        value.addProperty("crouching", entity.isCrouching());
        value.addProperty("sprinting", entity.isSprinting());
        value.addProperty("in_water", entity.isInWater());
        value.addProperty("under_water", entity.isUnderWater());
        value.addProperty("swimming", entity.isSwimming());
        value.addProperty("touching_unloaded_chunk", entity.touchingUnloadedChunk());
        value.addProperty("water_fluid_height", entity.getFluidHeight(FluidTags.WATER));
        value.addProperty(
            "level_has_chunk",
            entity.level().hasChunkAt(entity.getBlockX(), entity.getBlockZ())
        );
        JsonObject boundingBox = new JsonObject();
        boundingBox.addProperty("min_x", entity.getBoundingBox().minX);
        boundingBox.addProperty("min_y", entity.getBoundingBox().minY);
        boundingBox.addProperty("min_z", entity.getBoundingBox().minZ);
        boundingBox.addProperty("max_x", entity.getBoundingBox().maxX);
        boundingBox.addProperty("max_y", entity.getBoundingBox().maxY);
        boundingBox.addProperty("max_z", entity.getBoundingBox().maxZ);
        value.add("bounding_box", boundingBox);

        Vec3 velocity = entity.getDeltaMovement();
        JsonObject velocityJson = new JsonObject();
        velocityJson.addProperty("x", velocity.x);
        velocityJson.addProperty("y", velocity.y);
        velocityJson.addProperty("z", velocity.z);
        value.add("velocity", velocityJson);
        if (entity instanceof LivingEntity living) {
            value.addProperty("health", living.getHealth());
            value.addProperty("max_health", living.getMaxHealth());
        }
        if (entity instanceof ItemEntity itemEntity) {
            value.add("item", item(itemEntity.getItem()));
        }
        return value;
    }

    private static JsonArray inventory(Minecraft minecraft) {
        JsonArray inventory = new JsonArray();
        int selected = minecraft.player.getInventory().getSelectedSlot();
        for (int slot = 0; slot < minecraft.player.getInventory().getContainerSize(); slot++) {
            ItemStack stack = minecraft.player.getInventory().getItem(slot);
            if (stack.isEmpty()) {
                continue;
            }
            JsonObject entry = item(stack);
            entry.addProperty("slot", slot);
            entry.addProperty("selected", slot == selected);
            inventory.add(entry);
        }
        return inventory;
    }

    private static JsonObject container(AbstractContainerMenu menu) {
        JsonObject container = new JsonObject();
        container.addProperty("container_id", menu.containerId);
        container.addProperty("menu_class", menu.getClass().getName());
        container.addProperty("slot_count", menu.slots.size());
        JsonArray slots = new JsonArray();
        for (int slot = 0; slot < menu.slots.size(); slot++) {
            ItemStack stack = menu.getSlot(slot).getItem();
            if (stack.isEmpty()) {
                continue;
            }
            JsonObject entry = item(stack);
            entry.addProperty("slot", slot);
            slots.add(entry);
        }
        container.add("slots", slots);
        container.add("carried", item(menu.getCarried()));
        return container;
    }

    private static JsonObject item(ItemStack stack) {
        JsonObject item = new JsonObject();
        JsonArray enchantments = new JsonArray();
        item.add("enchantments", enchantments);
        if (stack.isEmpty()) {
            item.addProperty("item_id", "minecraft:air");
            item.addProperty("count", 0);
            return item;
        }
        item.addProperty("item_id", BuiltInRegistries.ITEM.getKey(stack.getItem()).toString());
        item.addProperty("count", stack.getCount());
        item.addProperty("name", stack.getHoverName().getString());
        item.addProperty("damage", stack.getDamageValue());
        item.addProperty("max_damage", stack.getMaxDamage());
        item.addProperty("foil", stack.hasFoil());
        stack.getEnchantments().entrySet().stream()
            .sorted(Comparator.comparing(entry -> entry.getKey().getRegisteredName()))
            .forEach(entry -> {
                JsonObject enchantment = new JsonObject();
                enchantment.addProperty("id", entry.getKey().getRegisteredName());
                enchantment.addProperty("level", entry.getIntValue());
                enchantments.add(enchantment);
            });
        return item;
    }

    private static JsonObject screen(Minecraft minecraft) {
        JsonObject screen = new JsonObject();
        if (minecraft.screen == null) {
            screen.addProperty("open", false);
            screen.addProperty("class", "none");
            screen.addProperty("title", "");
            return screen;
        }
        screen.addProperty("open", true);
        screen.addProperty("class", minecraft.screen.getClass().getName());
        screen.addProperty("title", minecraft.screen.getTitle().getString());
        return screen;
    }

    private static JsonObject target(Minecraft minecraft) {
        JsonObject target = new JsonObject();
        HitResult hit = minecraft.hitResult;
        if (hit == null) {
            target.addProperty("type", "none");
            return target;
        }
        target.addProperty("type", hit.getType().name().toLowerCase(Locale.ROOT));
        target.add("location", vector(hit.getLocation()));
        if (hit instanceof BlockHitResult blockHit) {
            BlockPos position = blockHit.getBlockPos();
            target.addProperty("x", position.getX());
            target.addProperty("y", position.getY());
            target.addProperty("z", position.getZ());
            target.addProperty("face", blockHit.getDirection().getName());
            if (minecraft.level.isLoaded(position)) {
                target.addProperty(
                    "block_id",
                    BuiltInRegistries.BLOCK.getKey(minecraft.level.getBlockState(position).getBlock()).toString()
                );
            }
        } else if (hit instanceof EntityHitResult entityHit) {
            Entity entity = entityHit.getEntity();
            addEntityIdentity(target, new ScenarioEntityIdentity(
                entity.getId(),
                entity.getUUID(),
                BuiltInRegistries.ENTITY_TYPE.getKey(entity.getType()).toString()
            ));
        }
        return target;
    }

    static JsonObject entityIdentity(ScenarioEntityIdentity identity) {
        JsonObject value = new JsonObject();
        addEntityIdentity(value, identity);
        return value;
    }

    private static void addEntityIdentity(JsonObject target, ScenarioEntityIdentity identity) {
        target.addProperty("entity_id", identity.entityId());
        target.addProperty("entity_uuid", identity.entityUuid().toString());
        target.addProperty("entity_type", identity.entityType());
    }

    private static JsonArray recentChat(Minecraft minecraft) {
        JsonArray recent = new JsonArray();
        List<String> messages = chatMessageTexts(minecraft.gui.getChat());
        for (int index = 0; index < Math.min(RECENT_CHAT_LIMIT, messages.size()); index++) {
            recent.add(messages.get(index));
        }
        return recent;
    }

    private static List<String> chatMessageTexts(ChatComponent chat) {
        try {
            Field allMessagesField = ChatComponent.class.getDeclaredField("allMessages");
            allMessagesField.setAccessible(true);
            Object value = allMessagesField.get(chat);
            if (!(value instanceof List<?> messages)) {
                throw new IllegalStateException("ChatComponent.allMessages is not a List");
            }
            List<String> texts = new ArrayList<>();
            for (Object message : messages) {
                if (message instanceof GuiMessage guiMessage) {
                    texts.add(guiMessage.content().getString());
                }
            }
            return texts;
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("could not read client chat history", error);
        }
    }

    private static JsonObject vector(Vec3 vector) {
        JsonObject value = new JsonObject();
        value.addProperty("x", vector.x);
        value.addProperty("y", vector.y);
        value.addProperty("z", vector.z);
        return value;
    }

    private static JsonObject bounds(
        int minX,
        int minY,
        int minZ,
        int maxX,
        int maxY,
        int maxZ
    ) {
        JsonObject bounds = new JsonObject();
        bounds.addProperty("min_x", minX);
        bounds.addProperty("min_y", minY);
        bounds.addProperty("min_z", minZ);
        bounds.addProperty("max_x", maxX);
        bounds.addProperty("max_y", maxY);
        bounds.addProperty("max_z", maxZ);
        return bounds;
    }

    private static int boundedVolume(
        int minX,
        int minY,
        int minZ,
        int maxX,
        int maxY,
        int maxZ,
        int maxBlocks
    ) {
        if (minX > maxX || minY > maxY || minZ > maxZ || maxBlocks < 1 || maxBlocks > 4096) {
            throw new IllegalArgumentException("invalid bounded client scan");
        }
        long sizeX = (long) maxX - minX + 1L;
        long sizeY = (long) maxY - minY + 1L;
        long sizeZ = (long) maxZ - minZ + 1L;
        if (sizeX > maxBlocks
            || sizeY > maxBlocks
            || sizeZ > maxBlocks
            || sizeX > maxBlocks / sizeY
            || sizeX * sizeY > maxBlocks / sizeZ) {
            throw new IllegalArgumentException("scan volume exceeds the bounded client query limit");
        }
        return (int) (sizeX * sizeY * sizeZ);
    }

    private record VisibleEntity(Entity entity, double distanceSquared) {
    }
}
