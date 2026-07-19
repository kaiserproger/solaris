package dev.solaris.agent.javaagent;

import dev.solaris.agent.client.ClientScenarioReport;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

final class PlayableRealClientLoopScenario {
    static final String JOIN_ID = "playable-01-join-generated-spawn";
    static final String WOOD_TO_TOOL_ID = "playable-02-natural-wood-to-tool";
    static final String LOG_TO_PLANKS_ID = "playable-02a-natural-log-to-planks";
    static final String CRAFTING_TABLE_OPEN_ID = "playable-02b-natural-crafting-table-open";
    static final String SAVE_RESTART_ID = "playable-03-save-restart-rejoin";
    static final String SAVE_RESTART_BEFORE_ID = "playable-03-save-restart-before";
    static final String SAVE_RESTART_AFTER_ID = "playable-03-save-restart-after";
    static final String TWENTY_MINUTE_ID = "playable-04-twenty-minute-survival-loop";
    static final String STONE_TOOL_ID = "playable-05-stone-tool-progression";
    static final String STONE_TOOL_SAVE_RESTART_ID = "playable-06-stone-tool-save-restart";
    static final String STONE_TOOL_SAVE_RESTART_BEFORE_ID = "playable-06-stone-tool-save-restart-before";
    static final String STONE_TOOL_SAVE_RESTART_AFTER_ID = "playable-06-stone-tool-save-restart-after";
    static final String FURNACE_PLACEMENT_OPEN_ID = "playable-07-furnace-placement-open";
    static final String FURNACE_CHARCOAL_SMELT_ID = "playable-08-furnace-charcoal-smelt";
    static final String TORCH_CRAFT_PLACE_ID = "playable-09-torch-craft-place";
    static final String PASSIVE_FOOD_DROP_ID = "playable-10-passive-food-drop";
    static final String EAT_PASSIVE_FOOD_ID = "playable-11-eat-passive-food";
    static final String EARNED_CHEST_STORAGE_ID = "playable-12-earned-chest-storage";
    static final String CHEST_STORAGE_SAVE_RESTART_ID = "playable-13-chest-storage-save-restart";
    static final String CHEST_STORAGE_SAVE_RESTART_BEFORE_ID = "playable-13-chest-storage-save-restart-before";
    static final String CHEST_STORAGE_SAVE_RESTART_AFTER_ID = "playable-13-chest-storage-save-restart-after";
    static final String EARNED_BED_SLEEP_ID = "playable-14-earned-bed-sleep";
    static final String COOKED_PASSIVE_FOOD_ID = "playable-15-cooked-passive-food";
    static final String EARNED_DOOR_PLACE_TOGGLE_ID = "playable-16-earned-door-place-toggle";
    static final String EARNED_SIGN_PLACE_EDIT_ID = "playable-17-earned-sign-place-edit";
    static final String EARNED_CAMPFIRE_COOKING_ID = "playable-18-earned-campfire-cooking";
    static final String EARNED_CAMPFIRE_DEATH_RESPAWN_ID = "playable-19-earned-campfire-death-respawn";
    static final String CAMPFIRE_DEATH_DROP_RECOVERY_ID = "playable-20-campfire-death-drop-recovery";
    static final String EARNED_TOOL_ZOMBIE_COMBAT_ID = "playable-21-earned-tool-zombie-combat";
    static final String STONE_SWORD_ZOMBIE_COMBAT_ID = "playable-22-stone-sword-zombie-combat";
    static final String IRON_INGOT_PROGRESSION_ID = "playable-23-iron-ingot-progression";
    static final String IRON_SWORD_ZOMBIE_COMBAT_ID = "playable-24-iron-sword-zombie-combat";
    static final String IRON_SWORD_SAVE_RESTART_ID = "playable-25-iron-sword-save-restart";
    static final String IRON_SWORD_SAVE_RESTART_BEFORE_ID = "playable-25-iron-sword-save-restart-before";
    static final String IRON_SWORD_SAVE_RESTART_AFTER_ID = "playable-25-iron-sword-save-restart-after";
    static final String EARNED_SHIELD_ZOMBIE_BLOCK_ID = "playable-26-earned-shield-zombie-block";
    static final String EARNED_IRON_CHESTPLATE_EQUIP_ID = "playable-27-earned-iron-chestplate-equip";
    static final String EARNED_IRON_CHESTPLATE_ZOMBIE_MITIGATION_ID =
        "playable-28-earned-iron-chestplate-zombie-mitigation";
    static final String IRON_CHESTPLATE_SAVE_RESTART_MITIGATION_ID =
        "playable-29-iron-chestplate-save-restart-mitigation";
    static final String IRON_CHESTPLATE_SAVE_RESTART_MITIGATION_BEFORE_ID =
        "playable-29-iron-chestplate-save-restart-mitigation-before";
    static final String IRON_CHESTPLATE_SAVE_RESTART_MITIGATION_AFTER_ID =
        "playable-29-iron-chestplate-save-restart-mitigation-after";
    static final String TWO_CLIENT_SHARED_LOG_DROP_PICKUP_ID =
        "playable-30-two-client-shared-log-drop-pickup";
    static final String TWO_CLIENT_SHARED_LOG_DROP_BREAK_ID =
        "playable-30-two-client-shared-log-drop-break";
    static final String TWO_CLIENT_SHARED_LOG_DROP_OBSERVE_ID =
        "playable-30-two-client-shared-log-drop-observe";
    static final String TWO_CLIENT_SHARED_LOG_PICKUP_COLLECT_ID =
        "playable-30-two-client-shared-log-pickup-collect";
    static final String TWO_CLIENT_SHARED_LOG_PICKUP_GONE_OBSERVE_ID =
        "playable-30-two-client-shared-log-pickup-gone-observe";
    static final String TWO_CLIENT_EARNED_SHARED_CHEST_ID =
        "playable-31-two-client-earned-shared-chest";
    static final String TWO_CLIENT_EARNED_SHARED_CHEST_DEPOSIT_ID =
        "playable-31-two-client-earned-shared-chest-deposit";
    static final String TWO_CLIENT_EARNED_SHARED_CHEST_WITHDRAW_ID =
        "playable-31-two-client-earned-shared-chest-withdraw";
    static final String TWO_CLIENT_EARNED_SHARED_CHEST_OBSERVE_EMPTY_ID =
        "playable-31-two-client-earned-shared-chest-observe-empty";
    static final String TWO_CLIENT_EARNED_TORCH_BLOCK_EDIT_ID =
        "playable-32-two-client-earned-torch-block-edit";
    static final String TWO_CLIENT_EARNED_TORCH_PLACE_ID =
        "playable-32-two-client-earned-torch-place";
    static final String TWO_CLIENT_EARNED_TORCH_OBSERVE_ID =
        "playable-32-two-client-earned-torch-observe";
    static final String TWO_CLIENT_EARNED_TORCH_BREAK_ID =
        "playable-32-two-client-earned-torch-break";
    static final String TWO_CLIENT_EARNED_TORCH_GONE_OBSERVE_ID =
        "playable-32-two-client-earned-torch-gone-observe";
    static final String TWO_CLIENT_PLAYER_VISIBILITY_MOVEMENT_ID =
        "playable-33-two-client-player-visibility-movement";
    static final String TWO_CLIENT_PLAYER_OBSERVE_ID =
        "playable-33-two-client-player-observe";
    static final String TWO_CLIENT_PLAYER_MOVED_OBSERVE_ID =
        "playable-33-two-client-player-moved-observe";
    static final String TWO_CLIENT_CHAT_MESSAGE_ID =
        "playable-34-two-client-chat-message";
    static final String TWO_CLIENT_CHAT_SEND_ID =
        "playable-34-two-client-chat-send";
    static final String TWO_CLIENT_CHAT_OBSERVE_ID =
        "playable-34-two-client-chat-observe";
    static final String TWO_CLIENT_PLAYER_DISCONNECT_REMOVAL_ID =
        "playable-35-two-client-player-disconnect-removal";
    static final String TWO_CLIENT_PLAYER_DISCONNECT_VISIBLE_ID =
        "playable-35-two-client-player-disconnect-visible";
    static final String TWO_CLIENT_PLAYER_GONE_OBSERVE_ID =
        "playable-35-two-client-player-gone-observe";
    static final String TWO_CLIENT_PLAYER_RECONNECT_CLEANUP_ID =
        "playable-36-two-client-player-reconnect-cleanup";
    static final String TWO_CLIENT_PLAYER_RECONNECT_VISIBLE_ID =
        "playable-36-two-client-player-reconnect-visible";
    static final String TWO_CLIENT_PLAYER_RECONNECT_GONE_OBSERVE_ID =
        "playable-36-two-client-player-reconnect-gone-observe";
    static final String TWO_CLIENT_PLAYER_RECONNECTED_OBSERVE_ID =
        "playable-36-two-client-player-reconnected-observe";
    static final String TWO_CLIENT_PLAYER_DEATH_RESPAWN_VISIBILITY_ID =
        "playable-37-two-client-player-death-respawn-visibility";
    static final String TWO_CLIENT_PLAYER_DEATH_BASELINE_ID =
        "playable-37-two-client-player-death-baseline";
    static final String TWO_CLIENT_CAMPFIRE_DEATH_RESPAWN_ID =
        "playable-37-two-client-campfire-death-respawn";
    static final String TWO_CLIENT_PLAYER_POST_RESPAWN_MOVED_OBSERVE_ID =
        "playable-37-two-client-player-post-respawn-moved-observe";
    static final String TWO_CLIENT_INVENTORY_DROP_HANDOFF_ID =
        "playable-38-two-client-inventory-drop-handoff";
    static final String TWO_CLIENT_INVENTORY_DROP_PRIMARY_ID =
        "playable-38-two-client-inventory-drop-primary";
    static final String TWO_CLIENT_INVENTORY_DROP_OBSERVE_ID =
        "playable-38-two-client-inventory-drop-observe";
    static final String TWO_CLIENT_INVENTORY_DROP_SECONDARY_PICKUP_ID =
        "playable-38-two-client-inventory-drop-secondary-pickup";
    static final String TWO_CLIENT_INVENTORY_DROP_GONE_OBSERVE_ID =
        "playable-38-two-client-inventory-drop-gone-observe";
    static final String RENEWABLE_WHEAT_BREAD_ID = "playable-43-renewable-wheat-bread";
    static final String PASSIVE_LIVESTOCK_MOTION_ID = "playable-44-passive-livestock-motion";
    static final String GENERATED_RUIN_CACHE_ID = "playable-46-generated-ruin-cache";
    static final String GENERATED_RUIN_CACHE_BEFORE_ID = "playable-46-generated-ruin-cache-before";
    static final String GENERATED_RUIN_CACHE_AFTER_ID = "playable-46-generated-ruin-cache-after";
    static final String STONECUTTER_CONSERVATION_ID = "playable-47-stonecutter-conservation";
    private static final String SAVE_RESTART_MARKER_FILE = "playable-03-save-restart-marker.properties";
    private static final String CHEST_STORAGE_MARKER_FILE = "playable-13-chest-storage-marker.properties";
    private static final String SHARED_LOG_DROP_MARKER_FILE = "playable-30-shared-log-drop-marker.properties";
    private static final String SHARED_CHEST_MARKER_FILE = "playable-31-shared-chest-marker.properties";
    private static final String SHARED_BLOCK_EDIT_MARKER_FILE =
        "playable-32-shared-block-edit-marker.properties";
    private static final String PLAYER_VISIBILITY_MARKER_FILE =
        "playable-33-player-visibility-marker.properties";
    private static final String INVENTORY_DROP_MARKER_FILE =
        "playable-38-inventory-drop-marker.properties";
    private static final String GENERATED_RUIN_CACHE_MARKER_FILE =
        "playable-46-generated-ruin-cache-marker.properties";
    private static final String PRIMARY_CLIENT_USERNAME = "SolarisPrimary";
    private static final String TWO_CLIENT_CHAT_MESSAGE_TEXT = "p34 hello from primary";
    private static final double PLAYER_MOVEMENT_MIN_HORIZONTAL_DELTA = 0.05;
    private static final int CHEST_RECIPE_DISPLAY_ID = 5;
    private static final int CRAFTING_TABLE_RECIPE_DISPLAY_ID = 10;
    private static final int FURNACE_RECIPE_DISPLAY_ID = 13;
    private static final int STICK_RECIPE_DISPLAY_ID = 21;
    private static final int STONE_PICKAXE_RECIPE_DISPLAY_ID = 24;
    private static final int STONE_SWORD_RECIPE_DISPLAY_ID = 26;
    private static final int TORCH_RECIPE_DISPLAY_ID = 27;
    private static final int WOODEN_PICKAXE_RECIPE_DISPLAY_ID = 31;
    private static final int WHITE_BED_RECIPE_DISPLAY_ID = 34;
    private static final String EARNED_BED_WOOL_ITEM_ID = "minecraft:white_wool";
    private static final String EARNED_BED_ITEM_ID = "minecraft:white_bed";
    private static final int CAMPFIRE_RECIPE_DISPLAY_ID = 53;
    private static final int IRON_SWORD_RECIPE_DISPLAY_ID = 57;
    private static final int SHIELD_RECIPE_DISPLAY_ID = 58;
    private static final int IRON_CHESTPLATE_RECIPE_DISPLAY_ID = 59;
    private static final int WOODEN_HOE_RECIPE_DISPLAY_ID = 30;
    private static final int BREAD_RECIPE_DISPLAY_ID = 60;
    private static final String CONTAINER_SCREEN = "net.minecraft.client.gui.screens.inventory.ContainerScreen";
    private static final String CRAFTING_SCREEN = "net.minecraft.client.gui.screens.inventory.CraftingScreen";
    private static final String FURNACE_SCREEN = "net.minecraft.client.gui.screens.inventory.FurnaceScreen";
    private static final String STONECUTTER_SCREEN =
        "net.minecraft.client.gui.screens.inventory.StonecutterScreen";
    private static final String STONECUTTER_INPUT_ITEM_ID = "minecraft:cobblestone";
    private static final String STONECUTTER_OUTPUT_ITEM_ID = "minecraft:cobblestone_slab";
    private static final int STONECUTTER_COBBLESTONE_OFFER_COUNT = 3;
    private static final int STONECUTTER_INPUT_SLOT = 0;
    private static final int STONECUTTER_OUTPUT_SLOT = 1;
    private static final Duration BREAK_TIMEOUT = Duration.ofSeconds(10);
    private static final Duration PICKUP_TIMEOUT = Duration.ofSeconds(8);
    private static final Duration INVENTORY_TIMEOUT = Duration.ofSeconds(5);
    private static final Duration FURNACE_COOK_TIMEOUT = Duration.ofSeconds(20);
    private static final Duration CAMPFIRE_COOK_TIMEOUT = Duration.ofSeconds(45);
    private static final Duration CAMPFIRE_DEATH_TIMEOUT = Duration.ofSeconds(45);
    private static final Duration RESPAWN_TIMEOUT = Duration.ofSeconds(10);
    private static final Duration BLOCK_TIMEOUT = Duration.ofSeconds(2);
    private static final Duration HOTBAR_TIMEOUT = Duration.ofSeconds(3);
    private static final Duration APPROACH_TIMEOUT = Duration.ofSeconds(30);
    private static final int GENERATED_RUIN_CENTER_X = 72;
    private static final int GENERATED_RUIN_CENTER_Z = 8;
    private static final int GENERATED_RUIN_CHEST_SLOT_COUNT = 27;
    private static final List<String> GENERATED_RUIN_SUPPORTED_FACES = List.of(
        "up",
        "down",
        "north",
        "south",
        "west",
        "east"
    );
    private static final List<GeneratedRuinLoot> GENERATED_RUIN_LOOT = List.of(
        new GeneratedRuinLoot("minecraft:diamond", 1, -1),
        new GeneratedRuinLoot("minecraft:lapis_lazuli", 4, -1),
        new GeneratedRuinLoot("minecraft:bread", 2, -1)
    );
    private static final Duration ENTITY_SCAN_TIMEOUT = Duration.ofSeconds(20);
    private static final Duration CHAT_TIMEOUT = Duration.ofSeconds(10);
    private static final Duration ENTITY_ATTACK_TIMEOUT = Duration.ofSeconds(20);
    private static final Duration HUNGER_DRAIN_TIMEOUT = Duration.ofSeconds(120);
    private static final Duration FOOD_EAT_TIMEOUT = Duration.ofSeconds(8);
    private static final Duration SHIELD_BLOCK_TIMEOUT = Duration.ofSeconds(8);
    private static final Duration HOSTILE_HIT_TIMEOUT = Duration.ofSeconds(10);
    private static final Duration TWENTY_MINUTE_SOAK = Duration.ofMinutes(20);
    private static final Duration NIGHT_WAIT_TIMEOUT = Duration.ofMinutes(12);
    private static final Duration MORNING_WAIT_TIMEOUT = Duration.ofSeconds(8);
    private static final Duration CROP_GROWTH_TIMEOUT = Duration.ofMinutes(20);
    private static final Duration LIVESTOCK_MOTION_TIMEOUT = Duration.ofSeconds(90);
    private static final double LIVESTOCK_MIN_HORIZONTAL_DISTANCE = 1.0;
    private static final double COW_MIN_VERTICAL_RISE = 0.8;
    private static final double LIVESTOCK_MIN_HORIZONTAL_SPEED = 0.02;
    private static final double LIVESTOCK_MAX_HORIZONTAL_SPEED = 0.25;
    private static final double LIVESTOCK_MAX_YAW_DELTA = 15.0;
    private static final float IRON_CHESTPLATE_MAX_ZOMBIE_HIT_DAMAGE = 2.75F;
    private static final long NIGHT_START_DAY_TIME = 12_542L;
    private static final Map<String, PlanksRecipe> PLANKS_BY_LOG = Map.ofEntries(
        Map.entry("minecraft:acacia_log", new PlanksRecipe("minecraft:acacia_log", "minecraft:acacia_planks", 0)),
        Map.entry("minecraft:birch_log", new PlanksRecipe("minecraft:birch_log", "minecraft:birch_planks", 2)),
        Map.entry("minecraft:cherry_log", new PlanksRecipe("minecraft:cherry_log", "minecraft:cherry_planks", 5)),
        Map.entry(
            "minecraft:dark_oak_log",
            new PlanksRecipe("minecraft:dark_oak_log", "minecraft:dark_oak_planks", 12)
        ),
        Map.entry("minecraft:jungle_log", new PlanksRecipe("minecraft:jungle_log", "minecraft:jungle_planks", 16)),
        Map.entry(
            "minecraft:mangrove_log",
            new PlanksRecipe("minecraft:mangrove_log", "minecraft:mangrove_planks", 17)
        ),
        Map.entry("minecraft:oak_log", new PlanksRecipe("minecraft:oak_log", "minecraft:oak_planks", 18)),
        Map.entry(
            "minecraft:pale_oak_log",
            new PlanksRecipe("minecraft:pale_oak_log", "minecraft:pale_oak_planks", 19)
        ),
        Map.entry("minecraft:spruce_log", new PlanksRecipe("minecraft:spruce_log", "minecraft:spruce_planks", 20))
    );
    private static final Map<String, DoorRecipe> DOOR_BY_PLANKS = Map.ofEntries(
        Map.entry("minecraft:acacia_planks", new DoorRecipe("minecraft:acacia_door", 35)),
        Map.entry("minecraft:birch_planks", new DoorRecipe("minecraft:birch_door", 36)),
        Map.entry("minecraft:cherry_planks", new DoorRecipe("minecraft:cherry_door", 37)),
        Map.entry("minecraft:dark_oak_planks", new DoorRecipe("minecraft:dark_oak_door", 38)),
        Map.entry("minecraft:jungle_planks", new DoorRecipe("minecraft:jungle_door", 39)),
        Map.entry("minecraft:mangrove_planks", new DoorRecipe("minecraft:mangrove_door", 40)),
        Map.entry("minecraft:oak_planks", new DoorRecipe("minecraft:oak_door", 41)),
        Map.entry("minecraft:pale_oak_planks", new DoorRecipe("minecraft:pale_oak_door", 42)),
        Map.entry("minecraft:spruce_planks", new DoorRecipe("minecraft:spruce_door", 43))
    );
    private static final Map<String, SignRecipe> SIGN_BY_PLANKS = Map.ofEntries(
        Map.entry("minecraft:acacia_planks", new SignRecipe("minecraft:acacia_sign", 44)),
        Map.entry("minecraft:birch_planks", new SignRecipe("minecraft:birch_sign", 45)),
        Map.entry("minecraft:cherry_planks", new SignRecipe("minecraft:cherry_sign", 46)),
        Map.entry("minecraft:dark_oak_planks", new SignRecipe("minecraft:dark_oak_sign", 47)),
        Map.entry("minecraft:jungle_planks", new SignRecipe("minecraft:jungle_sign", 48)),
        Map.entry("minecraft:mangrove_planks", new SignRecipe("minecraft:mangrove_sign", 49)),
        Map.entry("minecraft:oak_planks", new SignRecipe("minecraft:oak_sign", 50)),
        Map.entry("minecraft:pale_oak_planks", new SignRecipe("minecraft:pale_oak_sign", 51)),
        Map.entry("minecraft:spruce_planks", new SignRecipe("minecraft:spruce_sign", 52))
    );
    private static final List<String> P17_SIGN_LINES = List.of("Solaris", "P17", "NoDebug", "OK");
    private static final List<String> SUPPORTED_LOG_BLOCK_IDS = List.of(
        "minecraft:oak_log",
        "minecraft:spruce_log",
        "minecraft:birch_log",
        "minecraft:jungle_log",
        "minecraft:acacia_log",
        "minecraft:dark_oak_log",
        "minecraft:mangrove_log",
        "minecraft:cherry_log",
        "minecraft:pale_oak_log"
    );
    private static final Map<String, String> PASSIVE_FOOD_DROPS = Map.of(
        "minecraft:cow", "minecraft:beef",
        "minecraft:pig", "minecraft:porkchop",
        "minecraft:chicken", "minecraft:chicken"
    );
    private static final Map<String, String> COOKED_PASSIVE_FOOD_RESULTS = Map.of(
        "minecraft:beef", "minecraft:cooked_beef",
        "minecraft:porkchop", "minecraft:cooked_porkchop",
        "minecraft:chicken", "minecraft:cooked_chicken"
    );
    private static final List<String> PASSIVE_FOOD_ENTITY_IDS = List.of(
        "minecraft:cow",
        "minecraft:pig",
        "minecraft:chicken"
    );
    private static final List<String> SHEEP_WOOL_ENTITY_IDS = List.of(
        "minecraft:sheep"
    );
    private static final List<String> ZOMBIE_ENTITY_IDS = List.of(
        "minecraft:zombie"
    );
    private static final List<String> HOSTILE_ENTITY_IDS = List.of(
        "minecraft:zombie",
        "minecraft:skeleton",
        "minecraft:spider"
    );
    private static final Duration SURVIVAL_SOAK_STEP_TIMEOUT = Duration.ofSeconds(30);
    private static final long SURVIVAL_RESOURCE_INTERVAL_TICKS = 500L;
    private static final long SURVIVAL_RESOURCE_RETRY_TICKS = 100L;
    private static final int MAX_CONSECUTIVE_RESOURCE_BLOCKS = 3;
    private static final List<String> IRON_ORE_BLOCK_IDS = List.of(
        "minecraft:iron_ore",
        "minecraft:deepslate_iron_ore"
    );
    private final Duration survivalSoakDuration;

    private record PlanksRecipe(String logItemId, String planksItemId, int recipeDisplayId) {}

    private record DoorRecipe(String doorItemId, int recipeDisplayId) {}

    private record SignRecipe(String signItemId, int recipeDisplayId) {}

    private record LogToPlanksResult(ClientScenarioReport report, PlanksRecipe planks) {}

    private record StoneToolProgressionResult(ClientScenarioReport report, ScenarioBlockTarget tableTarget) {}

    private record WoodenToolTableResult(
        ClientScenarioReport report,
        PlanksRecipe planks,
        ScenarioBlockTarget tableTarget
    ) {}

    private record FurnacePlacementOpenResult(
        ClientScenarioReport report,
        PlanksRecipe planks,
        ScenarioBlockTarget furnaceTarget
    ) {}

    private record IronProgressionBaseResult(
        ClientScenarioReport report,
        WoodenToolTableResult prepared,
        FurnacePlacementOpenResult placedFurnace
    ) {}

    private record IronSwordProgressionResult(
        ClientScenarioReport report,
        ScenarioBlockTarget tableTarget
    ) {}

    private record IronChestplateProgressionResult(
        ClientScenarioReport report,
        ScenarioBlockTarget tableTarget
    ) {}
    private record SharedLogDropMarker(ScenarioBlockTarget target, String itemId) {}

    private record SharedBlockEditMarker(
        ScenarioBlockTarget target,
        ScenarioBlockTarget approachTarget,
        String blockId,
        String itemId
    ) {}

    private record PlayerVisibilityMarker(ScenarioPlayerObservation observation) {}

    private record FurnaceCharcoalSmeltResult(
        ClientScenarioReport report,
        PlanksRecipe planks,
        ScenarioBlockTarget furnaceTarget
    ) {}

    private record TorchPlacementResult(
        ClientScenarioReport report,
        ScenarioBlockTarget torchTarget,
        ScenarioBlockTarget approachTarget
    ) {}

    private record PassiveFoodDropResult(ClientScenarioReport report, String dropItemId, int foodCountAfter) {}

    private record BedRecipe(String woolItemId, String bedItemId, int recipeDisplayId) {}

    private record WoolCollectionResult(ClientScenarioReport report, BedRecipe bedRecipe) {}

    private record ChestStorageResult(
        ClientScenarioReport report,
        ScenarioBlockTarget chestTarget,
        String itemId,
        int count
    ) {}

    private record ChestStorageMarker(ScenarioBlockTarget chestTarget, String itemId, int count) {}

    private record GeneratedRuinLoot(String itemId, int count, int slot) {}

    private record GeneratedRuinCacheMarker(
        ScenarioBlockTarget chestTarget,
        List<GeneratedRuinLoot> loot
    ) {}

    private record CampfireDeathRespawnResult(
        ClientScenarioReport report,
        ScenarioBlockTarget campfireTarget,
        ScenarioItemDropIdentity woodenPickaxeDropIdentity
    ) {}

    private record CampfireCookingResult(
        ClientScenarioReport report,
        ScenarioBlockTarget campfireTarget
    ) {}

    PlayableRealClientLoopScenario() {
        this(TWENTY_MINUTE_SOAK);
    }

    PlayableRealClientLoopScenario(Duration survivalSoakDuration) {
        if (survivalSoakDuration.isZero() || survivalSoakDuration.isNegative()) {
            throw new IllegalArgumentException("survival soak duration must be positive");
        }
        this.survivalSoakDuration = survivalSoakDuration;
    }

    static boolean supports(String id) {
        return JOIN_ID.equals(id)
            || WOOD_TO_TOOL_ID.equals(id)
            || LOG_TO_PLANKS_ID.equals(id)
            || CRAFTING_TABLE_OPEN_ID.equals(id)
            || SAVE_RESTART_ID.equals(id)
            || SAVE_RESTART_BEFORE_ID.equals(id)
            || SAVE_RESTART_AFTER_ID.equals(id)
            || TWENTY_MINUTE_ID.equals(id)
            || STONE_TOOL_ID.equals(id)
            || STONE_TOOL_SAVE_RESTART_ID.equals(id)
            || STONE_TOOL_SAVE_RESTART_BEFORE_ID.equals(id)
            || STONE_TOOL_SAVE_RESTART_AFTER_ID.equals(id)
            || FURNACE_PLACEMENT_OPEN_ID.equals(id)
            || FURNACE_CHARCOAL_SMELT_ID.equals(id)
            || TORCH_CRAFT_PLACE_ID.equals(id)
            || PASSIVE_FOOD_DROP_ID.equals(id)
            || EAT_PASSIVE_FOOD_ID.equals(id)
            || EARNED_CHEST_STORAGE_ID.equals(id)
            || CHEST_STORAGE_SAVE_RESTART_ID.equals(id)
            || CHEST_STORAGE_SAVE_RESTART_BEFORE_ID.equals(id)
            || CHEST_STORAGE_SAVE_RESTART_AFTER_ID.equals(id)
            || EARNED_BED_SLEEP_ID.equals(id)
            || COOKED_PASSIVE_FOOD_ID.equals(id)
            || EARNED_DOOR_PLACE_TOGGLE_ID.equals(id)
            || EARNED_SIGN_PLACE_EDIT_ID.equals(id)
            || EARNED_CAMPFIRE_COOKING_ID.equals(id)
            || EARNED_CAMPFIRE_DEATH_RESPAWN_ID.equals(id)
            || CAMPFIRE_DEATH_DROP_RECOVERY_ID.equals(id)
            || EARNED_TOOL_ZOMBIE_COMBAT_ID.equals(id)
            || STONE_SWORD_ZOMBIE_COMBAT_ID.equals(id)
            || IRON_INGOT_PROGRESSION_ID.equals(id)
            || IRON_SWORD_ZOMBIE_COMBAT_ID.equals(id)
            || IRON_SWORD_SAVE_RESTART_ID.equals(id)
            || IRON_SWORD_SAVE_RESTART_BEFORE_ID.equals(id)
            || IRON_SWORD_SAVE_RESTART_AFTER_ID.equals(id)
            || EARNED_SHIELD_ZOMBIE_BLOCK_ID.equals(id)
            || EARNED_IRON_CHESTPLATE_EQUIP_ID.equals(id)
            || EARNED_IRON_CHESTPLATE_ZOMBIE_MITIGATION_ID.equals(id)
            || IRON_CHESTPLATE_SAVE_RESTART_MITIGATION_ID.equals(id)
            || IRON_CHESTPLATE_SAVE_RESTART_MITIGATION_BEFORE_ID.equals(id)
            || IRON_CHESTPLATE_SAVE_RESTART_MITIGATION_AFTER_ID.equals(id)
            || TWO_CLIENT_SHARED_LOG_DROP_PICKUP_ID.equals(id)
            || TWO_CLIENT_SHARED_LOG_DROP_BREAK_ID.equals(id)
            || TWO_CLIENT_SHARED_LOG_DROP_OBSERVE_ID.equals(id)
            || TWO_CLIENT_SHARED_LOG_PICKUP_COLLECT_ID.equals(id)
            || TWO_CLIENT_SHARED_LOG_PICKUP_GONE_OBSERVE_ID.equals(id)
            || TWO_CLIENT_EARNED_SHARED_CHEST_ID.equals(id)
            || TWO_CLIENT_EARNED_SHARED_CHEST_DEPOSIT_ID.equals(id)
            || TWO_CLIENT_EARNED_SHARED_CHEST_WITHDRAW_ID.equals(id)
            || TWO_CLIENT_EARNED_SHARED_CHEST_OBSERVE_EMPTY_ID.equals(id)
            || TWO_CLIENT_EARNED_TORCH_BLOCK_EDIT_ID.equals(id)
            || TWO_CLIENT_EARNED_TORCH_PLACE_ID.equals(id)
            || TWO_CLIENT_EARNED_TORCH_OBSERVE_ID.equals(id)
            || TWO_CLIENT_EARNED_TORCH_BREAK_ID.equals(id)
            || TWO_CLIENT_EARNED_TORCH_GONE_OBSERVE_ID.equals(id)
            || TWO_CLIENT_PLAYER_VISIBILITY_MOVEMENT_ID.equals(id)
            || TWO_CLIENT_PLAYER_OBSERVE_ID.equals(id)
            || TWO_CLIENT_PLAYER_MOVED_OBSERVE_ID.equals(id)
            || TWO_CLIENT_CHAT_MESSAGE_ID.equals(id)
            || TWO_CLIENT_CHAT_SEND_ID.equals(id)
            || TWO_CLIENT_CHAT_OBSERVE_ID.equals(id)
            || TWO_CLIENT_PLAYER_DISCONNECT_REMOVAL_ID.equals(id)
            || TWO_CLIENT_PLAYER_DISCONNECT_VISIBLE_ID.equals(id)
            || TWO_CLIENT_PLAYER_GONE_OBSERVE_ID.equals(id)
            || TWO_CLIENT_PLAYER_RECONNECT_CLEANUP_ID.equals(id)
            || TWO_CLIENT_PLAYER_RECONNECT_VISIBLE_ID.equals(id)
            || TWO_CLIENT_PLAYER_RECONNECT_GONE_OBSERVE_ID.equals(id)
            || TWO_CLIENT_PLAYER_RECONNECTED_OBSERVE_ID.equals(id)
            || TWO_CLIENT_PLAYER_DEATH_RESPAWN_VISIBILITY_ID.equals(id)
            || TWO_CLIENT_PLAYER_DEATH_BASELINE_ID.equals(id)
            || TWO_CLIENT_CAMPFIRE_DEATH_RESPAWN_ID.equals(id)
            || TWO_CLIENT_PLAYER_POST_RESPAWN_MOVED_OBSERVE_ID.equals(id)
            || TWO_CLIENT_INVENTORY_DROP_HANDOFF_ID.equals(id)
            || TWO_CLIENT_INVENTORY_DROP_PRIMARY_ID.equals(id)
            || TWO_CLIENT_INVENTORY_DROP_OBSERVE_ID.equals(id)
            || TWO_CLIENT_INVENTORY_DROP_SECONDARY_PICKUP_ID.equals(id)
            || TWO_CLIENT_INVENTORY_DROP_GONE_OBSERVE_ID.equals(id)
            || RENEWABLE_WHEAT_BREAD_ID.equals(id)
            || PASSIVE_LIVESTOCK_MOTION_ID.equals(id)
            || GENERATED_RUIN_CACHE_BEFORE_ID.equals(id)
            || GENERATED_RUIN_CACHE_AFTER_ID.equals(id)
            || STONECUTTER_CONSERVATION_ID.equals(id);
    }

    ClientScenarioReport run(String id, Path screenshotsDir, ScenarioClient client) {
        if (!supports(id)) {
            return new ClientScenarioReport("blocked", id, List.of("unsupported scenario id: " + id));
        }

        List<String> observations = new ArrayList<>();
        try {
            ScenarioHeldItem selected = client.selectedItem();
            observations.add("join/play-state: passed selected=" + selected.itemId() + " x" + selected.count());
            observations.add(
                "inventory baseline: oak_log="
                    + client.inventoryCount("minecraft:oak_log")
                    + " oak_planks="
                    + client.inventoryCount("minecraft:oak_planks")
                    + " crafting_table="
                    + client.inventoryCount("minecraft:crafting_table")
                    + " wooden_pickaxe="
                    + client.inventoryCount("minecraft:wooden_pickaxe")
            );
            observations.add("artifact directory available to driver: " + screenshotsDir);

            if (JOIN_ID.equals(id)) {
                return new ClientScenarioReport("passed", id, observations);
            }
            if (LOG_TO_PLANKS_ID.equals(id)) {
                return runWoodToToolStart(id, observations, client, true);
            }
            if (CRAFTING_TABLE_OPEN_ID.equals(id)) {
                return runCraftingTableOpen(id, observations, client);
            }
            if (WOOD_TO_TOOL_ID.equals(id)) {
                return runWoodToTool(id, observations, client);
            }
            if (SAVE_RESTART_BEFORE_ID.equals(id)) {
                return runSaveRestartBefore(id, observations, screenshotsDir, client);
            }
            if (SAVE_RESTART_AFTER_ID.equals(id)) {
                return runSaveRestartAfter(id, observations, screenshotsDir, client);
            }
            if (TWENTY_MINUTE_ID.equals(id)) {
                return runTwentyMinuteSurvivalLoop(id, observations, screenshotsDir, client);
            }
            if (STONE_TOOL_ID.equals(id)) {
                return runStoneToolProgression(id, observations, client).report();
            }
            if (STONE_TOOL_SAVE_RESTART_BEFORE_ID.equals(id)) {
                return runStoneToolSaveRestartBefore(id, observations, screenshotsDir, client);
            }
            if (STONE_TOOL_SAVE_RESTART_AFTER_ID.equals(id)) {
                return runStoneToolSaveRestartAfter(id, observations, screenshotsDir, client);
            }
            if (FURNACE_PLACEMENT_OPEN_ID.equals(id)) {
                return runFurnacePlacementOpen(id, observations, client);
            }
            if (FURNACE_CHARCOAL_SMELT_ID.equals(id)) {
                return runFurnaceCharcoalSmelt(id, observations, client);
            }
            if (TORCH_CRAFT_PLACE_ID.equals(id)) {
                return runTorchCraftPlace(id, observations, client);
            }
            if (PASSIVE_FOOD_DROP_ID.equals(id)) {
                return runPassiveFoodDrop(id, observations, client);
            }
            if (EAT_PASSIVE_FOOD_ID.equals(id)) {
                return runEatPassiveFood(id, observations, client);
            }
            if (RENEWABLE_WHEAT_BREAD_ID.equals(id)) {
                return runRenewableWheatBread(id, observations, client);
            }
            if (PASSIVE_LIVESTOCK_MOTION_ID.equals(id)) {
                return runPassiveLivestockMotion(id, observations, client);
            }
            if (EARNED_CHEST_STORAGE_ID.equals(id)) {
                return runEarnedChestStorage(id, observations, client);
            }
            if (CHEST_STORAGE_SAVE_RESTART_BEFORE_ID.equals(id)) {
                return runChestStorageSaveRestartBefore(id, observations, screenshotsDir, client);
            }
            if (CHEST_STORAGE_SAVE_RESTART_AFTER_ID.equals(id)) {
                return runChestStorageSaveRestartAfter(id, observations, screenshotsDir, client);
            }
            if (GENERATED_RUIN_CACHE_BEFORE_ID.equals(id)) {
                return runGeneratedRuinCacheBefore(id, observations, screenshotsDir, client);
            }
            if (GENERATED_RUIN_CACHE_AFTER_ID.equals(id)) {
                return runGeneratedRuinCacheAfter(id, observations, screenshotsDir, client);
            }
            if (STONECUTTER_CONSERVATION_ID.equals(id)) {
                return runStonecutterConservation(id, observations, client);
            }
            if (EARNED_BED_SLEEP_ID.equals(id)) {
                return runEarnedBedSleep(id, observations, client);
            }
            if (COOKED_PASSIVE_FOOD_ID.equals(id)) {
                return runCookedPassiveFood(id, observations, client);
            }
            if (EARNED_DOOR_PLACE_TOGGLE_ID.equals(id)) {
                return runEarnedDoorPlaceToggle(id, observations, client);
            }
            if (EARNED_SIGN_PLACE_EDIT_ID.equals(id)) {
                return runEarnedSignPlaceEdit(id, observations, client);
            }
            if (EARNED_CAMPFIRE_COOKING_ID.equals(id)) {
                return runEarnedCampfireCooking(id, observations, client);
            }
            if (EARNED_CAMPFIRE_DEATH_RESPAWN_ID.equals(id)) {
                return runEarnedCampfireDeathRespawn(id, observations, client);
            }
            if (CAMPFIRE_DEATH_DROP_RECOVERY_ID.equals(id)) {
                return runCampfireDeathDropRecovery(id, observations, client);
            }
            if (EARNED_TOOL_ZOMBIE_COMBAT_ID.equals(id)) {
                return runEarnedToolZombieCombat(id, observations, client);
            }
            if (STONE_SWORD_ZOMBIE_COMBAT_ID.equals(id)) {
                return runStoneSwordZombieCombat(id, observations, client);
            }
            if (IRON_INGOT_PROGRESSION_ID.equals(id)) {
                return runIronIngotProgression(id, observations, client);
            }
            if (IRON_SWORD_ZOMBIE_COMBAT_ID.equals(id)) {
                return runIronSwordZombieCombat(id, observations, client);
            }
            if (IRON_SWORD_SAVE_RESTART_BEFORE_ID.equals(id)) {
                return runIronSwordSaveRestartBefore(id, observations, screenshotsDir, client);
            }
            if (IRON_SWORD_SAVE_RESTART_AFTER_ID.equals(id)) {
                return runIronSwordSaveRestartAfter(id, observations, screenshotsDir, client);
            }
            if (EARNED_SHIELD_ZOMBIE_BLOCK_ID.equals(id)) {
                return runEarnedShieldZombieBlock(id, observations, client);
            }
            if (EARNED_IRON_CHESTPLATE_EQUIP_ID.equals(id)) {
                return runEarnedIronChestplateEquip(id, observations, client);
            }
            if (EARNED_IRON_CHESTPLATE_ZOMBIE_MITIGATION_ID.equals(id)) {
                return runEarnedIronChestplateZombieMitigation(id, observations, client);
            }
            if (IRON_CHESTPLATE_SAVE_RESTART_MITIGATION_BEFORE_ID.equals(id)) {
                return runIronChestplateSaveRestartMitigationBefore(id, observations, screenshotsDir, client);
            }
            if (IRON_CHESTPLATE_SAVE_RESTART_MITIGATION_AFTER_ID.equals(id)) {
                return runIronChestplateSaveRestartMitigationAfter(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_SHARED_LOG_DROP_PICKUP_ID.equals(id)) {
                return new ClientScenarioReport(
                    "blocked",
                    id,
                    List.of("blocked: playable-30 requires runner-managed primary and secondary real-client phases")
                );
            }
            if (TWO_CLIENT_SHARED_LOG_DROP_BREAK_ID.equals(id)) {
                return runTwoClientSharedLogDropBreak(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_SHARED_LOG_DROP_OBSERVE_ID.equals(id)) {
                return runTwoClientSharedLogDropObserve(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_SHARED_LOG_PICKUP_COLLECT_ID.equals(id)) {
                return runTwoClientSharedLogPickupCollect(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_SHARED_LOG_PICKUP_GONE_OBSERVE_ID.equals(id)) {
                return runTwoClientSharedLogPickupGoneObserve(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_EARNED_SHARED_CHEST_ID.equals(id)) {
                return new ClientScenarioReport(
                    "blocked",
                    id,
                    List.of("blocked: playable-31 requires runner-managed primary and secondary real-client phases")
                );
            }
            if (TWO_CLIENT_EARNED_SHARED_CHEST_DEPOSIT_ID.equals(id)) {
                return runTwoClientEarnedSharedChestDeposit(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_EARNED_SHARED_CHEST_WITHDRAW_ID.equals(id)) {
                return runTwoClientEarnedSharedChestWithdraw(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_EARNED_SHARED_CHEST_OBSERVE_EMPTY_ID.equals(id)) {
                return runTwoClientEarnedSharedChestObserveEmpty(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_EARNED_TORCH_BLOCK_EDIT_ID.equals(id)) {
                return new ClientScenarioReport(
                    "blocked",
                    id,
                    List.of("blocked: playable-32 requires runner-managed primary and secondary real-client phases")
                );
            }
            if (TWO_CLIENT_EARNED_TORCH_PLACE_ID.equals(id)) {
                return runTwoClientEarnedTorchPlace(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_EARNED_TORCH_OBSERVE_ID.equals(id)) {
                return runTwoClientEarnedTorchObserve(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_EARNED_TORCH_BREAK_ID.equals(id)) {
                return runTwoClientEarnedTorchBreak(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_EARNED_TORCH_GONE_OBSERVE_ID.equals(id)) {
                return runTwoClientEarnedTorchGoneObserve(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_PLAYER_VISIBILITY_MOVEMENT_ID.equals(id)) {
                return new ClientScenarioReport(
                    "blocked",
                    id,
                    List.of("blocked: playable-33 requires runner-managed primary and secondary real-client phases")
                );
            }
            if (TWO_CLIENT_PLAYER_OBSERVE_ID.equals(id)) {
                return runTwoClientPlayerObserve(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_PLAYER_MOVED_OBSERVE_ID.equals(id)) {
                return runTwoClientPlayerMovedObserve(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_CHAT_MESSAGE_ID.equals(id)) {
                return new ClientScenarioReport(
                    "blocked",
                    id,
                    List.of("blocked: playable-34 requires runner-managed primary and secondary real-client phases")
                );
            }
            if (TWO_CLIENT_CHAT_SEND_ID.equals(id)) {
                return runTwoClientChatSend(id, observations, client);
            }
            if (TWO_CLIENT_CHAT_OBSERVE_ID.equals(id)) {
                return runTwoClientChatObserve(id, observations, client);
            }
            if (TWO_CLIENT_PLAYER_DISCONNECT_REMOVAL_ID.equals(id)) {
                return new ClientScenarioReport(
                    "blocked",
                    id,
                    List.of("blocked: playable-35 requires runner-managed primary and secondary real-client phases")
                );
            }
            if (TWO_CLIENT_PLAYER_DISCONNECT_VISIBLE_ID.equals(id)) {
                return runTwoClientPlayerDisconnectVisible(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_PLAYER_GONE_OBSERVE_ID.equals(id)) {
                return runTwoClientPlayerGoneObserve(id, observations, client);
            }
            if (TWO_CLIENT_PLAYER_RECONNECT_CLEANUP_ID.equals(id)) {
                return new ClientScenarioReport(
                    "blocked",
                    id,
                    List.of("blocked: playable-36 requires runner-managed primary reconnect and secondary real-client phases")
                );
            }
            if (TWO_CLIENT_PLAYER_RECONNECT_VISIBLE_ID.equals(id)) {
                return runTwoClientPlayerReconnectVisible(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_PLAYER_RECONNECT_GONE_OBSERVE_ID.equals(id)) {
                return runTwoClientPlayerReconnectGoneObserve(id, observations, client);
            }
            if (TWO_CLIENT_PLAYER_RECONNECTED_OBSERVE_ID.equals(id)) {
                return runTwoClientPlayerReconnectedObserve(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_PLAYER_DEATH_RESPAWN_VISIBILITY_ID.equals(id)) {
                return new ClientScenarioReport(
                    "blocked",
                    id,
                    List.of("blocked: playable-37 requires runner-managed primary death/respawn and secondary real-client phases")
                );
            }
            if (TWO_CLIENT_PLAYER_DEATH_BASELINE_ID.equals(id)) {
                return runTwoClientPlayerDeathBaseline(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_CAMPFIRE_DEATH_RESPAWN_ID.equals(id)) {
                return runTwoClientCampfireDeathRespawn(id, observations, client);
            }
            if (TWO_CLIENT_PLAYER_POST_RESPAWN_MOVED_OBSERVE_ID.equals(id)) {
                return runTwoClientPlayerPostRespawnMovedObserve(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_INVENTORY_DROP_HANDOFF_ID.equals(id)) {
                return new ClientScenarioReport(
                    "blocked",
                    id,
                    List.of("blocked: playable-38 requires runner-managed primary and secondary inventory-drop phases")
                );
            }
            if (TWO_CLIENT_INVENTORY_DROP_PRIMARY_ID.equals(id)) {
                return runTwoClientInventoryDropPrimary(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_INVENTORY_DROP_OBSERVE_ID.equals(id)) {
                return runTwoClientInventoryDropObserve(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_INVENTORY_DROP_SECONDARY_PICKUP_ID.equals(id)) {
                return runTwoClientInventoryDropSecondaryPickup(id, observations, screenshotsDir, client);
            }
            if (TWO_CLIENT_INVENTORY_DROP_GONE_OBSERVE_ID.equals(id)) {
                return runTwoClientInventoryDropGoneObserve(id, observations, screenshotsDir, client);
            }
            observations.add(
                "blocked: playable natural survival automation is intentionally no-debug; "
                    + "runner-managed save/restart and 20-minute loop "
                    + "need real-client natural action primitives"
            );
            observations.add(
                "blocked: debug give, teleport setup, protocol harnesses, and mock clients are forbidden for "
                    + id
            );
            return new ClientScenarioReport("blocked", id, observations);
        } catch (Exception error) {
            observations.add("scenario failed with exception: " + error.getMessage());
            return new ClientScenarioReport("failed", id, observations);
        }
    }

    private ClientScenarioReport runWoodToToolStart(
        String id,
        List<String> observations,
        ScenarioClient client,
        boolean terminal
    ) throws Exception {
        return runLogToPlanks(id, observations, client, 1, false, terminal).report();
    }

    private ClientScenarioReport runCraftingTableOpen(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        LogToPlanksResult logToPlanks = runLogToPlanks(id, observations, client, 1, false, false);
        if (!"passed".equals(logToPlanks.report().result())) {
            return logToPlanks.report();
        }

        return craftPlaceAndOpenTable(id, observations, client, logToPlanks.planks().planksItemId(), true)
            .report();
    }

    private LogToPlanksResult runLogToPlanks(
        String id,
        List<String> observations,
        ScenarioClient client,
        int targetLogCount,
        boolean useMaxItems,
        boolean terminal
    ) throws Exception {
        PlanksRecipe planks = null;
        int collected = 0;
        int attempts = 0;
        int maxAttempts = Math.max(3, targetLogCount * 4);
        while (collected < targetLogCount && attempts < maxAttempts) {
            attempts++;
            List<String> candidateLogs = planks == null ? SUPPORTED_LOG_BLOCK_IDS : List.of(planks.logItemId());
            ScenarioBlockTarget log = client.findBreakableBlock(
                candidateLogs,
                ScenarioReach.WITHIN_SURVIVAL_REACH
            );
            if (log == null) {
                ScenarioBlockTarget farLog = client.findBreakableBlock(
                    candidateLogs,
                    ScenarioReach.OUTSIDE_SURVIVAL_REACH
                );
                if (farLog == null) {
                    observations.add("blocked: no loaded supported natural log found near the real client");
                    return new LogToPlanksResult(new ClientScenarioReport("blocked", id, observations), planks);
                }
                boolean approached = client.approachBlock(farLog, APPROACH_TIMEOUT);
                observations.add(
                    "natural log approach: " + (approached ? "passed" : "failed")
                        + " target=" + coordinates(farLog)
                );
                log = client.findBreakableBlock(candidateLogs, ScenarioReach.WITHIN_SURVIVAL_REACH);
                if (!approached) {
                    if (log == null) {
                        return new LogToPlanksResult(new ClientScenarioReport("blocked", id, observations), planks);
                    }
                    observations.add(
                        "natural log reachable fallback after failed approach: passed"
                            + " target=" + coordinates(log)
                    );
                } else if (log == null) {
                    observations.add("blocked: supported natural log remained outside survival reach after approach");
                    return new LogToPlanksResult(new ClientScenarioReport("blocked", id, observations), planks);
                }
            }
            boolean closeApproached = true;
            if (usesReachOnlyLogClose(id) || "down".equals(log.face())) {
                observations.add(
                    "natural log close approach: skipped target=" + coordinates(log)
                        + " reason=already_within_survival_reach"
                );
            } else {
                closeApproached = client.approachBlock(log, APPROACH_TIMEOUT);
                observations.add(
                    "natural log close approach: " + (closeApproached ? "passed" : "failed")
                        + " target=" + coordinates(log)
                );
                if (!closeApproached) {
                    return new LogToPlanksResult(new ClientScenarioReport("blocked", id, observations), planks);
                }
            }
            PlanksRecipe detected = PLANKS_BY_LOG.get(log.blockId());
            if (detected == null) {
                observations.add("blocked: loaded log has no embedded planks recipe mapping block_id=" + log.blockId());
                return new LogToPlanksResult(new ClientScenarioReport("blocked", id, observations), planks);
            }
            if (planks == null) {
                planks = detected;
            }

            ScenarioBreakResult broke = client.breakBlockUntilDropVisible(log, log.blockId(), BREAK_TIMEOUT);
            ScenarioBreakResult pickup = client.collectVisibleItemDrop(log, log.blockId(), 1, PICKUP_TIMEOUT);
            boolean naturalPickup = broke.started()
                && broke.becameAir()
                && broke.sawDrop()
                && pickup.pickupRestored();
            observations.add(
                "natural log break/drop/pickup: " + (naturalPickup ? "passed" : "failed")
                    + " target=" + coordinates(log)
                    + " break_started=" + broke.started()
                    + " became_air=" + broke.becameAir()
                    + " saw_drop=" + broke.sawDrop()
                    + " pickup_restored=" + pickup.pickupRestored()
                    + " held=" + pickup.selectedItem().itemId() + " x" + pickup.selectedItem().count()
            );
            if (!naturalPickup) {
                continue;
            }
            collected++;
        }

        if (planks == null) {
            observations.add("inventory recipe: failed no collected log family was detected");
            return new LogToPlanksResult(new ClientScenarioReport("failed", id, observations), null);
        }
        if (collected < targetLogCount) {
            observations.add(
                "natural log collection: failed collected=" + collected
                    + " target=" + targetLogCount
                    + " attempts=" + attempts
            );
            return new LogToPlanksResult(new ClientScenarioReport("failed", id, observations), planks);
        }
        int logCount = client.inventoryCount(planks.logItemId());
        int planksCount = client.inventoryCount(planks.planksItemId());
        if (logCount < targetLogCount) {
            observations.add("inventory recipe: failed fewer log items available than expected after pickup");
            return new LogToPlanksResult(new ClientScenarioReport("failed", id, observations), planks);
        }
        int craftedLogs = useMaxItems ? logCount : 1;
        int expectedLogCount = logCount - craftedLogs;
        int expectedPlanksCount = planksCount + craftedLogs * 4;
        client.placeRecipe(0, planks.recipeDisplayId(), useMaxItems);
        boolean logConsumed = client.waitForInventoryCount(
            planks.logItemId(),
            expectedLogCount,
            INVENTORY_TIMEOUT
        );
        boolean planksCreated = client.waitForInventoryCount(
            planks.planksItemId(),
            expectedPlanksCount,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "inventory recipe: " + (logConsumed && planksCreated ? "passed" : "failed")
                + " recipe_display_id=" + planks.recipeDisplayId()
                + " use_max_items=" + useMaxItems
                + " log_item=" + planks.logItemId()
                + " log_expected_count=" + expectedLogCount
                + " log_count_matched=" + logConsumed
                + " planks_item=" + planks.planksItemId()
                + " planks_expected_count=" + expectedPlanksCount
                + " planks_count_matched=" + planksCreated
        );
        if (!logConsumed || !planksCreated) {
            return new LogToPlanksResult(new ClientScenarioReport("failed", id, observations), planks);
        }

        if (terminal) {
            observations.add(
                "remaining: crafting table placement/opening, sticks, wooden pickaxe, save/restart, and "
                    + "20-minute soak still need natural real-client automation"
            );
        }
        return new LogToPlanksResult(new ClientScenarioReport("passed", id, observations), planks);
    }

    private CraftingTableOpenResult craftPlaceAndOpenTable(
        String id,
        List<String> observations,
        ScenarioClient client,
        String planksItemId,
        boolean closeAfterOpen
    ) throws Exception {
        int planksCount = client.inventoryCount(planksItemId);
        int craftingTableCount = client.inventoryCount("minecraft:crafting_table");
        if (planksCount < 4) {
            observations.add("crafting table recipe: failed fewer than four planks available");
            return new CraftingTableOpenResult(new ClientScenarioReport("failed", id, observations), null);
        }
        int expectedPlanksCount = planksCount - 4;
        int expectedCraftingTableCount = craftingTableCount + 1;
        client.placeRecipe(0, CRAFTING_TABLE_RECIPE_DISPLAY_ID, false);
        boolean planksConsumed = client.waitForInventoryCount(
            planksItemId,
            expectedPlanksCount,
            INVENTORY_TIMEOUT
        );
        boolean tableCreated = client.waitForInventoryCount(
            "minecraft:crafting_table",
            expectedCraftingTableCount,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "crafting table recipe: " + (planksConsumed && tableCreated ? "passed" : "failed")
                + " recipe_display_id=" + CRAFTING_TABLE_RECIPE_DISPLAY_ID
                + " planks_item=" + planksItemId
                + " planks_expected_count=" + expectedPlanksCount
                + " planks_count_matched=" + planksConsumed
                + " crafting_table_expected_count=" + expectedCraftingTableCount
                + " crafting_table_count_matched=" + tableCreated
        );
        if (!planksConsumed || !tableCreated) {
            return new CraftingTableOpenResult(new ClientScenarioReport("failed", id, observations), null);
        }

        ScenarioHeldItem table = client.selectHotbarItem("minecraft:crafting_table", 1, HOTBAR_TIMEOUT);
        if (!table.matches("minecraft:crafting_table", 1)) {
            observations.add(
                "blocked: crafted table exists but is not selectable from hotbar without inventory mutation"
            );
            return new CraftingTableOpenResult(new ClientScenarioReport("blocked", id, observations), null);
        }
        ScenarioBlockPair pair = client.findUnobstructedPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (pair == null) {
            observations.add("blocked: no loaded dry target found for crafting table placement");
            return new CraftingTableOpenResult(new ClientScenarioReport("blocked", id, observations), null);
        }
        observations.add(
            "crafting table placement target: clicked="
                + pair.clicked().x() + "," + pair.clicked().y() + "," + pair.clicked().z()
                + "/" + pair.clicked().face()
                + " clicked_block=" + pair.clicked().blockId()
                + " target=" + pair.target().x() + "," + pair.target().y() + "," + pair.target().z()
                + " target_block=" + pair.target().blockId()
        );
        ScenarioUseResult placeUse = client.useItemOn(pair.clicked(), table);
        boolean placed = client.waitForBlock(pair.target(), "minecraft:crafting_table", BLOCK_TIMEOUT);
        ScenarioBlockTarget tableTarget = new ScenarioBlockTarget(
            pair.target().x(),
            pair.target().y(),
            pair.target().z(),
            "up",
            pair.target().label(),
            "minecraft:crafting_table"
        );
        if (!placed) {
            observations.add(
                "crafting table open: failed"
                    + " place_use_result=" + placeUse.result()
                    + " placed=false"
            );
            return new CraftingTableOpenResult(new ClientScenarioReport("failed", id, observations), tableTarget);
        }
        ScenarioUseResult openUse = client.useItemOn(tableTarget, table);
        boolean opened = client.waitForScreenClassName(CRAFTING_SCREEN, INVENTORY_TIMEOUT);
        boolean closed = !closeAfterOpen || client.closeCurrentScreen(INVENTORY_TIMEOUT);
        boolean passed = placed && opened && closed;
        observations.add(
            "crafting table open: " + (passed ? "passed" : "failed")
                + " place_use_result=" + placeUse.result()
                + " placed=" + placed
                + " open_use_result=" + openUse.result()
                + " screen=" + CRAFTING_SCREEN
                + " screen_matched=" + opened
                + " closed=" + closed
        );
        if (!passed) {
            return new CraftingTableOpenResult(new ClientScenarioReport("failed", id, observations), tableTarget);
        }

        if (closeAfterOpen) {
            observations.add(
                "remaining: sticks, wooden pickaxe, save/restart, and 20-minute soak still need natural real-client automation"
            );
        }
        return new CraftingTableOpenResult(new ClientScenarioReport("passed", id, observations), tableTarget);
    }

    private ClientScenarioReport runWoodToTool(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        LogToPlanksResult planks = runLogToPlanks(id, observations, client, 3, true, false);
        if (!"passed".equals(planks.report().result())) {
            return planks.report();
        }
        CraftingTableOpenResult table = craftPlaceAndOpenTable(
            id,
            observations,
            client,
            planks.planks().planksItemId(),
            false
        );
        if (!"passed".equals(table.report().result())) {
            return table.report();
        }

        ClientScenarioReport tool = craftWoodenPickaxeInOpenTable(
            id,
            observations,
            client,
            planks.planks().planksItemId(),
            true
        );
        if (!"passed".equals(tool.result())) {
            return tool;
        }

        observations.add("remaining: save/restart and 20-minute soak still need real-client automation");
        return new ClientScenarioReport("passed", id, observations);
    }

    private StoneToolProgressionResult runStoneToolProgression(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        LogToPlanksResult planks = runLogToPlanks(id, observations, client, 3, true, false);
        if (!"passed".equals(planks.report().result())) {
            return new StoneToolProgressionResult(planks.report(), null);
        }
        CraftingTableOpenResult table = craftPlaceAndOpenTable(
            id,
            observations,
            client,
            planks.planks().planksItemId(),
            false
        );
        if (!"passed".equals(table.report().result())) {
            return new StoneToolProgressionResult(table.report(), table.tableTarget());
        }
        ClientScenarioReport woodenTool = craftWoodenPickaxeInOpenTable(
            id,
            observations,
            client,
            planks.planks().planksItemId(),
            true
        );
        if (!"passed".equals(woodenTool.result())) {
            return new StoneToolProgressionResult(woodenTool, table.tableTarget());
        }

        ClientScenarioReport cobblestone = mineCobblestoneWithWoodenPickaxe(id, observations, client, 3);
        if (!"passed".equals(cobblestone.result())) {
            return new StoneToolProgressionResult(cobblestone, table.tableTarget());
        }

        boolean tableApproached = client.approachBlock(table.tableTarget(), APPROACH_TIMEOUT);
        observations.add(
            "crafting table approach for stone tool: " + (tableApproached ? "passed" : "failed")
                + " target=" + coordinates(table.tableTarget())
        );
        if (!tableApproached) {
            return new StoneToolProgressionResult(
                new ClientScenarioReport("blocked", id, observations),
                table.tableTarget()
            );
        }

        ScenarioHeldItem woodenPickaxe = client.selectHotbarItem("minecraft:wooden_pickaxe", 1, HOTBAR_TIMEOUT);
        ScenarioBlockTarget tableUseTarget = reachableUseTarget(client, table.tableTarget());
        ScenarioUseResult openUse = client.useItemOn(tableUseTarget, woodenPickaxe);
        boolean opened = client.waitForScreenClassName(CRAFTING_SCREEN, INVENTORY_TIMEOUT);
        observations.add(
            "crafting table reopen for stone tool: " + (opened ? "passed" : "failed")
                + " open_use_result=" + openUse.result()
                + " screen=" + CRAFTING_SCREEN
                + " screen_matched=" + opened
        );
        if (!opened) {
            return new StoneToolProgressionResult(
                new ClientScenarioReport("failed", id, observations),
                table.tableTarget()
            );
        }

        return new StoneToolProgressionResult(
            craftStonePickaxeInOpenTable(id, observations, client, true),
            table.tableTarget()
        );
    }

    private ClientScenarioReport runFurnacePlacementOpen(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        return prepareFurnacePlacementOpen(id, observations, client).report();
    }

    private FurnacePlacementOpenResult prepareFurnacePlacementOpen(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        WoodenToolTableResult prepared = prepareWoodenToolAndTable(id, observations, client);
        if (!"passed".equals(prepared.report().result())) {
            return new FurnacePlacementOpenResult(prepared.report(), prepared.planks(), null);
        }
        return finishFurnacePlacementOpen(id, observations, client, prepared.planks(), prepared.tableTarget());
    }

    private WoodenToolTableResult prepareWoodenToolAndTable(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        LogToPlanksResult planks = runLogToPlanks(id, observations, client, 4, true, false);
        if (!"passed".equals(planks.report().result())) {
            return new WoodenToolTableResult(planks.report(), planks.planks(), null);
        }
        CraftingTableOpenResult table = craftPlaceAndOpenTable(
            id,
            observations,
            client,
            planks.planks().planksItemId(),
            false
        );
        if (!"passed".equals(table.report().result())) {
            return new WoodenToolTableResult(table.report(), planks.planks(), table.tableTarget());
        }
        ClientScenarioReport woodenTool = craftWoodenPickaxeInOpenTable(
            id,
            observations,
            client,
            planks.planks().planksItemId(),
            true
        );
        if (!"passed".equals(woodenTool.result())) {
            return new WoodenToolTableResult(woodenTool, planks.planks(), table.tableTarget());
        }

        return new WoodenToolTableResult(
            new ClientScenarioReport("passed", id, observations),
            planks.planks(),
            table.tableTarget()
        );
    }

    private FurnacePlacementOpenResult finishFurnacePlacementOpen(
        String id,
        List<String> observations,
        ScenarioClient client,
        PlanksRecipe planks,
        ScenarioBlockTarget tableTarget
    ) throws Exception {
        ClientScenarioReport cobblestone = mineCobblestoneWithWoodenPickaxe(id, observations, client, 8);
        if (!"passed".equals(cobblestone.result())) {
            return new FurnacePlacementOpenResult(cobblestone, planks, null);
        }

        boolean tableApproached = client.approachBlock(tableTarget, APPROACH_TIMEOUT);
        observations.add(
            "crafting table approach for furnace: " + (tableApproached ? "passed" : "failed")
                + " target=" + coordinates(tableTarget)
        );
        if (tableApproached) {
            ScenarioHeldItem woodenPickaxe = client.selectHotbarItem("minecraft:wooden_pickaxe", 1, HOTBAR_TIMEOUT);
            ScenarioBlockTarget tableUseTarget = reachableUseTarget(client, tableTarget);
            ScenarioUseResult openUse = client.useItemOn(tableUseTarget, woodenPickaxe);
            boolean opened = client.waitForScreenClassName(CRAFTING_SCREEN, INVENTORY_TIMEOUT);
            observations.add(
                "crafting table reopen for furnace: " + (opened ? "passed" : "failed")
                    + " open_use_result=" + openUse.result()
                    + " screen=" + CRAFTING_SCREEN
                    + " screen_matched=" + opened
            );
            if (!opened) {
                return new FurnacePlacementOpenResult(
                    new ClientScenarioReport("failed", id, observations),
                    planks,
                    null
                );
            }
        } else {
            CraftingTableOpenResult spareTable = craftPlaceAndOpenTable(
                id,
                observations,
                client,
                planks.planksItemId(),
                false
            );
            if (!"passed".equals(spareTable.report().result())) {
                return new FurnacePlacementOpenResult(spareTable.report(), planks, null);
            }
        }

        ClientScenarioReport furnace = craftFurnaceInOpenTable(id, observations, client, true);
        if (!"passed".equals(furnace.result())) {
            return new FurnacePlacementOpenResult(furnace, planks, null);
        }
        FurnacePlacementOpenResult placed = placeAndOpenFurnace(id, observations, client);
        return new FurnacePlacementOpenResult(placed.report(), planks, placed.furnaceTarget());
    }

    private ClientScenarioReport runFurnaceCharcoalSmelt(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        return prepareFurnaceCharcoalSmelt(id, observations, client).report();
    }

    private FurnaceCharcoalSmeltResult prepareFurnaceCharcoalSmelt(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        FurnacePlacementOpenResult furnace = prepareFurnacePlacementOpen(id, observations, client);
        if (!"passed".equals(furnace.report().result())) {
            return new FurnaceCharcoalSmeltResult(
                furnace.report(),
                furnace.planks(),
                furnace.furnaceTarget()
            );
        }

        return smeltCharcoalInPlacedFurnace(id, observations, client, furnace);
    }

    private FurnaceCharcoalSmeltResult smeltCharcoalInPlacedFurnace(
        String id,
        List<String> observations,
        ScenarioClient client,
        FurnacePlacementOpenResult furnace
    ) throws Exception {
        int existingLogCount = client.inventoryCount(furnace.planks().logItemId());
        if (existingLogCount < 1) {
            ClientScenarioReport log = collectNaturalLogItem(id, observations, client, furnace.planks());
            if (!"passed".equals(log.result())) {
                return new FurnaceCharcoalSmeltResult(log, furnace.planks(), furnace.furnaceTarget());
            }
        } else {
            observations.add(
                "furnace input log inventory: passed item=" + furnace.planks().logItemId()
                    + " count=" + existingLogCount
            );
        }

        boolean furnaceApproached = client.approachBlock(furnace.furnaceTarget(), APPROACH_TIMEOUT);
        observations.add(
            "furnace approach for charcoal: " + (furnaceApproached ? "passed" : "failed")
                + " target=" + coordinates(furnace.furnaceTarget())
        );
        if (!furnaceApproached) {
            return new FurnaceCharcoalSmeltResult(
                new ClientScenarioReport("blocked", id, observations),
                furnace.planks(),
                furnace.furnaceTarget()
            );
        }

        ScenarioHeldItem furnaceItem = client.selectHotbarItem("minecraft:furnace", 1, HOTBAR_TIMEOUT);
        ScenarioBlockTarget furnaceUseTarget = reachableUseTarget(client, furnace.furnaceTarget());
        ScenarioUseResult openUse = client.useItemOn(furnaceUseTarget, furnaceItem);
        boolean opened = client.waitForScreenClassName(FURNACE_SCREEN, INVENTORY_TIMEOUT);
        observations.add(
            "furnace reopen for charcoal: " + (opened ? "passed" : "failed")
                + " open_use_result=" + openUse.result()
                + " screen=" + FURNACE_SCREEN
                + " screen_matched=" + opened
        );
        if (!opened) {
            return new FurnaceCharcoalSmeltResult(
                new ClientScenarioReport("failed", id, observations),
                furnace.planks(),
                furnace.furnaceTarget()
            );
        }

        boolean inputMoved = client.moveSelectedItemToContainerSlot(
            0,
            furnace.planks().logItemId(),
            1,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "furnace input transfer: " + (inputMoved ? "passed" : "failed")
                + " slot=0 item=" + furnace.planks().logItemId()
        );
        if (!inputMoved) {
            return new FurnaceCharcoalSmeltResult(
                new ClientScenarioReport("failed", id, observations),
                furnace.planks(),
                furnace.furnaceTarget()
            );
        }

        boolean fuelMoved = client.moveSelectedItemToContainerSlot(
            1,
            furnace.planks().planksItemId(),
            1,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "furnace fuel transfer: " + (fuelMoved ? "passed" : "failed")
                + " slot=1 item=" + furnace.planks().planksItemId()
        );
        if (!fuelMoved) {
            return new FurnaceCharcoalSmeltResult(
                new ClientScenarioReport("failed", id, observations),
                furnace.planks(),
                furnace.furnaceTarget()
            );
        }

        boolean outputReady = client.waitForContainerSlot(2, "minecraft:charcoal", 1, FURNACE_COOK_TIMEOUT);
        observations.add(
            "furnace charcoal output: " + (outputReady ? "passed" : "failed")
                + " slot=2 item=minecraft:charcoal"
        );
        if (!outputReady) {
            return new FurnaceCharcoalSmeltResult(
                new ClientScenarioReport("failed", id, observations),
                furnace.planks(),
                furnace.furnaceTarget()
            );
        }

        boolean fuelRemainderCleared = client.moveContainerSlotToInventory(
            1,
            furnace.planks().planksItemId(),
            1,
            INVENTORY_TIMEOUT
        ) || client.waitForContainerSlotEmpty(1, INVENTORY_TIMEOUT);
        observations.add(
            "furnace fuel remainder clear: " + (fuelRemainderCleared ? "passed" : "failed")
                + " slot=1 item=" + furnace.planks().planksItemId()
        );
        if (!fuelRemainderCleared) {
            return new FurnaceCharcoalSmeltResult(
                new ClientScenarioReport("failed", id, observations),
                furnace.planks(),
                furnace.furnaceTarget()
            );
        }

        boolean inputRemainderCleared = client.moveContainerSlotToInventory(
            0,
            furnace.planks().logItemId(),
            1,
            INVENTORY_TIMEOUT
        ) || client.waitForContainerSlotEmpty(0, INVENTORY_TIMEOUT);
        observations.add(
            "furnace input remainder clear: " + (inputRemainderCleared ? "passed" : "failed")
                + " slot=0 item=" + furnace.planks().logItemId()
        );
        if (!inputRemainderCleared) {
            return new FurnaceCharcoalSmeltResult(
                new ClientScenarioReport("failed", id, observations),
                furnace.planks(),
                furnace.furnaceTarget()
            );
        }

        boolean outputTaken = client.moveContainerSlotToInventory(
            2,
            "minecraft:charcoal",
            1,
            INVENTORY_TIMEOUT
        );
        boolean charcoalInInventory = client.waitForInventoryCount(
            "minecraft:charcoal",
            1,
            INVENTORY_TIMEOUT
        );
        boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        observations.add(
            "furnace charcoal inventory: "
                + (outputTaken && charcoalInInventory && closed ? "passed" : "failed")
                + " output_taken=" + outputTaken
                + " charcoal_inventory_matched=" + charcoalInInventory
                + " closed=" + closed
            );
        if (!outputTaken || !charcoalInInventory || !closed) {
            return new FurnaceCharcoalSmeltResult(
                new ClientScenarioReport("failed", id, observations),
                furnace.planks(),
                furnace.furnaceTarget()
            );
        }
        return new FurnaceCharcoalSmeltResult(
            new ClientScenarioReport("passed", id, observations),
            furnace.planks(),
            furnace.furnaceTarget()
        );
    }

    private ClientScenarioReport runTorchCraftPlace(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        return prepareTorchCraftPlace(id, observations, client).report();
    }

    private TorchPlacementResult prepareTorchCraftPlace(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        FurnaceCharcoalSmeltResult charcoal = prepareFurnaceCharcoalSmelt(id, observations, client);
        if (!"passed".equals(charcoal.report().result())) {
            return new TorchPlacementResult(charcoal.report(), null, null);
        }

        int charcoalCount = client.inventoryCount("minecraft:charcoal");
        int stickCount = client.inventoryCount("minecraft:stick");
        int torchCount = client.inventoryCount("minecraft:torch");
        if (charcoalCount < 1 || stickCount < 1) {
            observations.add(
                "torch recipe: failed missing charcoal or sticks"
                    + " charcoal_count=" + charcoalCount
                    + " stick_count=" + stickCount
            );
            return new TorchPlacementResult(new ClientScenarioReport("failed", id, observations), null, null);
        }

        int expectedCharcoalCount = charcoalCount - 1;
        int expectedStickCount = stickCount - 1;
        int expectedTorchCount = torchCount + 4;
        client.placeRecipe(0, TORCH_RECIPE_DISPLAY_ID, false);
        boolean charcoalConsumed = client.waitForInventoryCount(
            "minecraft:charcoal",
            expectedCharcoalCount,
            INVENTORY_TIMEOUT
        );
        boolean stickConsumed = client.waitForInventoryCount(
            "minecraft:stick",
            expectedStickCount,
            INVENTORY_TIMEOUT
        );
        boolean torchesCreated = client.waitForInventoryCount(
            "minecraft:torch",
            expectedTorchCount,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "torch recipe: "
                + (charcoalConsumed && stickConsumed && torchesCreated ? "passed" : "failed")
                + " recipe_display_id=" + TORCH_RECIPE_DISPLAY_ID
                + " charcoal_expected_count=" + expectedCharcoalCount
                + " charcoal_count_matched=" + charcoalConsumed
                + " stick_expected_count=" + expectedStickCount
                + " stick_count_matched=" + stickConsumed
                + " torch_expected_count=" + expectedTorchCount
                + " torch_count_matched=" + torchesCreated
        );
        if (!charcoalConsumed || !stickConsumed || !torchesCreated) {
            return new TorchPlacementResult(new ClientScenarioReport("failed", id, observations), null, null);
        }

        ScenarioHeldItem torch = client.selectHotbarItem("minecraft:torch", expectedTorchCount, HOTBAR_TIMEOUT);
        if (!torch.matches("minecraft:torch", expectedTorchCount)) {
            observations.add("blocked: crafted torches exist but are not selectable from hotbar");
            return new TorchPlacementResult(new ClientScenarioReport("blocked", id, observations), null, null);
        }
        ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (pair == null) {
            observations.add("blocked: no loaded dry target found for torch placement");
            return new TorchPlacementResult(new ClientScenarioReport("blocked", id, observations), null, null);
        }
        ScenarioBlockTarget clicked = new ScenarioBlockTarget(
            pair.clicked().x(),
            pair.clicked().y(),
            pair.clicked().z(),
            pair.clicked().face(),
            "torch-clicked",
            pair.clicked().blockId()
        );
        ScenarioBlockTarget target = new ScenarioBlockTarget(
            pair.target().x(),
            pair.target().y(),
            pair.target().z(),
            "up",
            "torch-target",
            "minecraft:torch"
        );
        ScenarioUseResult placeUse = client.useItemOn(clicked, torch);
        boolean placed = client.waitForBlock(target, "minecraft:torch", BLOCK_TIMEOUT);
        observations.add(
            "torch placement: " + (placed ? "passed" : "failed")
                + " place_use_result=" + placeUse.result()
                + " target=" + coordinates(target)
        );
        return new TorchPlacementResult(
            new ClientScenarioReport(placed ? "passed" : "failed", id, observations),
            placed ? target : null,
            placed ? clicked : null
        );
    }

    private ClientScenarioReport runPassiveFoodDrop(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        return collectPassiveFoodDrop(id, observations, client).report();
    }

    private ClientScenarioReport runPassiveLivestockMotion(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        boolean passed = true;
        for (String entityTypeId : List.of("minecraft:cow", "minecraft:sheep", "minecraft:chicken")) {
            ScenarioEntityObservation entity = client.findVisibleEntity(
                List.of(entityTypeId),
                ScenarioReach.OUTSIDE_SURVIVAL_REACH,
                ENTITY_SCAN_TIMEOUT
            );
            if (entity == null) {
                entity = client.findVisibleEntity(
                    List.of(entityTypeId),
                    ScenarioReach.WITHIN_SURVIVAL_REACH,
                    ENTITY_SCAN_TIMEOUT
                );
            }
            if (entity == null) {
                observations.add("blocked: no loaded natural livestock visible entity=" + entityTypeId);
                return new ClientScenarioReport("blocked", id, observations);
            }

            double minimumVerticalRise = "minecraft:cow".equals(entityTypeId) ? COW_MIN_VERTICAL_RISE : 0.0;
            ScenarioEntityMotionObservation motion = client.waitForEntityMotion(
                entity,
                LIVESTOCK_MIN_HORIZONTAL_DISTANCE,
                minimumVerticalRise,
                LIVESTOCK_MOTION_TIMEOUT
            );
            if (motion == null) {
                observations.add(
                    "livestock motion: failed entity=" + entityTypeId + " entity_id=" + entity.entityId()
                        + " reason=entity-disappeared"
                );
                passed = false;
                continue;
            }

            boolean moved = motion.horizontalDistance() >= LIVESTOCK_MIN_HORIZONTAL_DISTANCE;
            boolean climbed = motion.verticalRise() >= minimumVerticalRise;
            boolean speedMatched = motion.maxHorizontalSpeed() >= LIVESTOCK_MIN_HORIZONTAL_SPEED
                && motion.maxHorizontalSpeed() <= LIVESTOCK_MAX_HORIZONTAL_SPEED;
            boolean yawMatched = motion.minimumYawDelta() <= LIVESTOCK_MAX_YAW_DELTA;
            boolean entityPassed = moved && climbed && speedMatched && yawMatched;
            passed &= entityPassed;
            observations.add(
                "livestock motion: " + (entityPassed ? "passed" : "failed")
                    + " entity=" + entityTypeId
                    + " entity_id=" + entity.entityId()
                    + " horizontal_distance=" + motion.horizontalDistance()
                    + " vertical_rise=" + motion.verticalRise()
                    + " max_horizontal_speed=" + motion.maxHorizontalSpeed()
                    + " minimum_yaw_delta=" + motion.minimumYawDelta()
                    + " end=" + motion.endX() + "," + motion.endY() + "," + motion.endZ()
            );
        }
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runEatPassiveFood(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        PassiveFoodDropResult drop = collectPassiveFoodDrop(id, observations, client);
        if (!"passed".equals(drop.report().result())) {
            return drop.report();
        }

        boolean hungerDrained = client.drainHungerBySprinting(HUNGER_DRAIN_TIMEOUT);
        observations.add("natural hunger drain: " + (hungerDrained ? "passed" : "failed"));
        if (!hungerDrained) {
            return new ClientScenarioReport("failed", id, observations);
        }

        int foodCountBefore = client.inventoryCount(drop.dropItemId());
        if (foodCountBefore < 1) {
            observations.add("passive food eating: failed no earned food item remained after hunger drain");
            return new ClientScenarioReport("failed", id, observations);
        }
        ScenarioHeldItem selectedFood = client.selectHotbarItem(drop.dropItemId(), 1, HOTBAR_TIMEOUT);
        if (!selectedFood.matches(drop.dropItemId(), 1)) {
            observations.add("blocked: earned passive food exists but is not selectable from hotbar");
            return new ClientScenarioReport("blocked", id, observations);
        }

        ScenarioFoodUseResult eaten = client.eatSelectedFood(drop.dropItemId(), foodCountBefore, FOOD_EAT_TIMEOUT);
        boolean passed = eaten.started()
            && eaten.foodBefore() < 20
            && eaten.foodAfter() > eaten.foodBefore()
            && eaten.itemCountAfter() < eaten.itemCountBefore();
        observations.add(
            "passive food eating: " + (passed ? "passed" : "failed")
                + " item=" + drop.dropItemId()
                + " started=" + eaten.started()
                + " food_before=" + eaten.foodBefore()
                + " food_after=" + eaten.foodAfter()
                + " item_count_before=" + eaten.itemCountBefore()
                + " item_count_after=" + eaten.itemCountAfter()
        );
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runRenewableWheatBread(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        LogToPlanksResult planks = runLogToPlanks(id, observations, client, 3, true, false);
        if (!"passed".equals(planks.report().result())) {
            return planks.report();
        }
        CraftingTableOpenResult table = craftPlaceAndOpenTable(
            id,
            observations,
            client,
            planks.planks().planksItemId(),
            false
        );
        if (!"passed".equals(table.report().result())) {
            return table.report();
        }

        int containerId = client.activeContainerId();
        int planksBeforeSticks = client.inventoryCount(planks.planks().planksItemId());
        int sticksBefore = client.inventoryCount("minecraft:stick");
        if (planksBeforeSticks < 2) {
            observations.add("renewable food stick recipe: failed fewer than two earned planks available");
            return new ClientScenarioReport("failed", id, observations);
        }
        client.placeRecipe(containerId, STICK_RECIPE_DISPLAY_ID, false);
        boolean stickPlanksConsumed = client.waitForInventoryCount(
            planks.planks().planksItemId(),
            planksBeforeSticks - 2,
            INVENTORY_TIMEOUT
        );
        boolean sticksCreated = client.waitForInventoryCount(
            "minecraft:stick",
            sticksBefore + 4,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "renewable food stick recipe: " + (stickPlanksConsumed && sticksCreated ? "passed" : "failed")
        );
        if (!stickPlanksConsumed || !sticksCreated) {
            return new ClientScenarioReport("failed", id, observations);
        }

        int planksBeforeHoe = client.inventoryCount(planks.planks().planksItemId());
        int sticksBeforeHoe = client.inventoryCount("minecraft:stick");
        int hoesBefore = client.inventoryCount("minecraft:wooden_hoe");
        if (planksBeforeHoe < 2 || sticksBeforeHoe < 2) {
            observations.add("wooden hoe recipe: failed missing earned planks or sticks");
            return new ClientScenarioReport("failed", id, observations);
        }
        client.placeRecipe(containerId, WOODEN_HOE_RECIPE_DISPLAY_ID, false);
        boolean hoePlanksConsumed = client.waitForInventoryCount(
            planks.planks().planksItemId(),
            planksBeforeHoe - 2,
            INVENTORY_TIMEOUT
        );
        boolean hoeSticksConsumed = client.waitForInventoryCount(
            "minecraft:stick",
            sticksBeforeHoe - 2,
            INVENTORY_TIMEOUT
        );
        boolean hoeCreated = client.waitForInventoryCount(
            "minecraft:wooden_hoe",
            hoesBefore + 1,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "wooden hoe recipe: " + (hoePlanksConsumed && hoeSticksConsumed && hoeCreated ? "passed" : "failed")
                + " recipe_display_id=" + WOODEN_HOE_RECIPE_DISPLAY_ID
        );
        if (!hoePlanksConsumed || !hoeSticksConsumed || !hoeCreated) {
            return new ClientScenarioReport("failed", id, observations);
        }
        if (!client.closeCurrentScreen(INVENTORY_TIMEOUT)) {
            observations.add("renewable food crafting screen close: failed");
            return new ClientScenarioReport("failed", id, observations);
        }

        List<ScenarioBlockTarget> crops = new ArrayList<>();
        for (int plot = 1; plot <= 3; plot++) {
            ScenarioBlockTarget grass = client.findBreakableBlock(
                List.of("minecraft:short_grass"),
                ScenarioReach.WITHIN_SURVIVAL_REACH
            );
            if (grass == null) {
                ScenarioBlockTarget farGrass = client.findBreakableBlock(
                    List.of("minecraft:short_grass"),
                    ScenarioReach.OUTSIDE_SURVIVAL_REACH
                );
                if (farGrass == null || !client.approachBlock(farGrass, APPROACH_TIMEOUT)) {
                    observations.add("blocked: no reachable generated short grass for plot=" + plot);
                    return new ClientScenarioReport("blocked", id, observations);
                }
                grass = client.findBreakableBlock(
                    List.of("minecraft:short_grass"),
                    ScenarioReach.WITHIN_SURVIVAL_REACH
                );
            }
            if (grass == null || !client.approachBlock(grass, APPROACH_TIMEOUT)) {
                observations.add("blocked: generated short grass remained outside reach for plot=" + plot);
                return new ClientScenarioReport("blocked", id, observations);
            }

            int seedsBefore = client.inventoryCount("minecraft:wheat_seeds");
            ScenarioBreakResult grassBreak = client.breakBlockUntilDropVisible(
                grass,
                "minecraft:wheat_seeds",
                BREAK_TIMEOUT
            );
            ScenarioBreakResult seedPickup = client.collectVisibleItemDrop(
                grass,
                "minecraft:wheat_seeds",
                1,
                PICKUP_TIMEOUT
            );
            int seedsAfter = client.inventoryCount("minecraft:wheat_seeds");
            boolean seedEarned = grassBreak.started()
                && grassBreak.becameAir()
                && grassBreak.sawDrop()
                && seedPickup.pickupRestored()
                && seedsAfter >= seedsBefore + 1;
            observations.add(
                "wheat seed source plot=" + plot + ": " + (seedEarned ? "passed" : "failed")
                    + " target=" + coordinates(grass)
                    + " seeds_before=" + seedsBefore
                    + " seeds_after=" + seedsAfter
            );
            if (!seedEarned) {
                return new ClientScenarioReport("failed", id, observations);
            }

            ScenarioBlockPair soil = client.findTillableSoil(ScenarioReach.WITHIN_SURVIVAL_REACH);
            if (soil == null || !"up".equals(soil.clicked().face())) {
                observations.add("blocked: no reachable natural tillable soil for plot=" + plot);
                return new ClientScenarioReport("blocked", id, observations);
            }
            ScenarioHeldItem hoe = client.selectHotbarItem("minecraft:wooden_hoe", 1, HOTBAR_TIMEOUT);
            if (!hoe.matches("minecraft:wooden_hoe", 1)) {
                observations.add("blocked: earned wooden hoe is not selectable for plot=" + plot);
                return new ClientScenarioReport("blocked", id, observations);
            }
            ScenarioUseResult tillUse = client.useItemOn(soil.clicked(), hoe);
            boolean tilled = client.waitForBlock(soil.clicked(), "minecraft:farmland", BLOCK_TIMEOUT);
            ScenarioBlockTarget farmland = new ScenarioBlockTarget(
                soil.clicked().x(),
                soil.clicked().y(),
                soil.clicked().z(),
                "up",
                "renewable-wheat-farmland-" + plot,
                "minecraft:farmland"
            );

            int seedsBeforePlant = client.inventoryCount("minecraft:wheat_seeds");
            ScenarioHeldItem seeds = client.selectHotbarItem("minecraft:wheat_seeds", 1, HOTBAR_TIMEOUT);
            if (!seeds.matches("minecraft:wheat_seeds", 1)) {
                observations.add("blocked: earned wheat seed is not selectable for plot=" + plot);
                return new ClientScenarioReport("blocked", id, observations);
            }
            ScenarioUseResult plantUse = client.useItemOn(farmland, seeds);
            ScenarioBlockTarget crop = new ScenarioBlockTarget(
                soil.target().x(),
                soil.target().y(),
                soil.target().z(),
                "up",
                "renewable-wheat-crop-" + plot,
                "minecraft:wheat"
            );
            boolean planted = client.waitForBlock(crop, "minecraft:wheat", BLOCK_TIMEOUT);
            boolean seedConsumed = client.waitForInventoryCount(
                "minecraft:wheat_seeds",
                seedsBeforePlant - 1,
                INVENTORY_TIMEOUT
            );
            observations.add(
                "wheat planting plot=" + plot + ": " + (tilled && planted && seedConsumed ? "passed" : "failed")
                    + " till_use=" + tillUse.result()
                    + " plant_use=" + plantUse.result()
            );
            if (!tilled || !planted || !seedConsumed) {
                return new ClientScenarioReport("failed", id, observations);
            }
            crops.add(crop);
        }

        for (int index = 0; index < crops.size(); index++) {
            boolean mature = client.waitForBlockProperty(
                crops.get(index),
                "age",
                "7",
                CROP_GROWTH_TIMEOUT
            );
            observations.add(
                "wheat natural growth plot=" + (index + 1) + ": " + (mature ? "passed" : "failed")
            );
            if (!mature) {
                return new ClientScenarioReport("failed", id, observations);
            }
            ScenarioLightLevel light = client.lightLevel(crops.get(index));
            boolean growthLit = Math.max(light.sky(), light.block()) >= 9;
            observations.add(
                "wheat client light plot=" + (index + 1) + ": "
                    + (growthLit ? "passed" : "failed")
                    + " sky=" + light.sky()
                    + " block=" + light.block()
            );
            if (!growthLit) {
                return new ClientScenarioReport("failed", id, observations);
            }
        }

        for (int index = 0; index < crops.size(); index++) {
            ScenarioBlockTarget crop = crops.get(index);
            ScenarioBlockTarget support = new ScenarioBlockTarget(
                crop.x(),
                crop.y() - 1,
                crop.z(),
                "up",
                "renewable-wheat-support-" + (index + 1),
                "minecraft:farmland"
            );
            if (!client.approachBlock(support, APPROACH_TIMEOUT)) {
                observations.add("blocked: mature wheat support remained outside reach plot=" + (index + 1));
                return new ClientScenarioReport("blocked", id, observations);
            }
            int wheatBefore = client.inventoryCount("minecraft:wheat");
            ScenarioBreakResult cropBreak = client.breakBlockUntilDropVisible(
                crop,
                "minecraft:wheat",
                BREAK_TIMEOUT
            );
            ScenarioBreakResult wheatPickup = client.collectVisibleItemDrop(
                crop,
                "minecraft:wheat",
                1,
                PICKUP_TIMEOUT
            );
            int wheatAfter = client.inventoryCount("minecraft:wheat");
            boolean harvested = cropBreak.started()
                && cropBreak.becameAir()
                && cropBreak.sawDrop()
                && wheatPickup.pickupRestored()
                && wheatAfter >= wheatBefore + 1;
            observations.add(
                "wheat harvest plot=" + (index + 1) + ": " + (harvested ? "passed" : "failed")
                    + " wheat_before=" + wheatBefore
                    + " wheat_after=" + wheatAfter
            );
            if (!harvested) {
                return new ClientScenarioReport("failed", id, observations);
            }
        }

        if (!client.approachBlock(table.tableTarget(), APPROACH_TIMEOUT)) {
            observations.add("blocked: crafting table remained outside reach after harvest");
            return new ClientScenarioReport("blocked", id, observations);
        }
        ScenarioHeldItem hoe = client.selectHotbarItem("minecraft:wooden_hoe", 1, HOTBAR_TIMEOUT);
        ScenarioUseResult openUse = client.useItemOn(reachableUseTarget(client, table.tableTarget()), hoe);
        boolean opened = client.waitForScreenClassName(CRAFTING_SCREEN, INVENTORY_TIMEOUT);
        observations.add(
            "crafting table reopen for bread: " + (opened ? "passed" : "failed")
                + " open_use_result=" + openUse.result()
        );
        if (!opened) {
            return new ClientScenarioReport("failed", id, observations);
        }

        containerId = client.activeContainerId();
        int wheatBeforeBread = client.inventoryCount("minecraft:wheat");
        int breadBefore = client.inventoryCount("minecraft:bread");
        if (wheatBeforeBread < 3) {
            observations.add("bread recipe: failed fewer than three earned wheat available");
            return new ClientScenarioReport("failed", id, observations);
        }
        client.placeRecipe(containerId, BREAD_RECIPE_DISPLAY_ID, false);
        boolean wheatConsumed = client.waitForInventoryCount(
            "minecraft:wheat",
            wheatBeforeBread - 3,
            INVENTORY_TIMEOUT
        );
        boolean breadCreated = client.waitForInventoryCount(
            "minecraft:bread",
            breadBefore + 1,
            INVENTORY_TIMEOUT
        );
        boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        observations.add(
            "bread recipe: " + (wheatConsumed && breadCreated && closed ? "passed" : "failed")
                + " recipe_display_id=" + BREAD_RECIPE_DISPLAY_ID
        );
        if (!wheatConsumed || !breadCreated || !closed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        observations.add("renewable bread ready: passed count=" + (breadBefore + 1));
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runCookedPassiveFood(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        WoodenToolTableResult prepared = prepareWoodenToolAndTable(id, observations, client);
        if (!"passed".equals(prepared.report().result())) {
            return prepared.report();
        }

        PassiveFoodDropResult drop = collectPassiveFoodDrop(id, observations, client);
        if (!"passed".equals(drop.report().result())) {
            return drop.report();
        }
        String cookedItemId = COOKED_PASSIVE_FOOD_RESULTS.get(drop.dropItemId());
        if (cookedItemId == null) {
            observations.add("blocked: passive food drop has no cooked furnace result item=" + drop.dropItemId());
            return new ClientScenarioReport("blocked", id, observations);
        }

        FurnacePlacementOpenResult placedFurnace = finishFurnacePlacementOpen(
            id,
            observations,
            client,
            prepared.planks(),
            prepared.tableTarget()
        );
        if (!"passed".equals(placedFurnace.report().result())) {
            return placedFurnace.report();
        }

        FurnaceCharcoalSmeltResult charcoal = smeltCharcoalInPlacedFurnace(id, observations, client, placedFurnace);
        if (!"passed".equals(charcoal.report().result())) {
            return charcoal.report();
        }

        boolean furnaceApproached = client.approachBlock(charcoal.furnaceTarget(), APPROACH_TIMEOUT);
        observations.add(
            "furnace approach for cooked food: " + (furnaceApproached ? "passed" : "failed")
                + " target=" + coordinates(charcoal.furnaceTarget())
        );
        if (!furnaceApproached) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        ScenarioBlockTarget furnaceUseTarget = reachableUseTarget(client, charcoal.furnaceTarget());
        ScenarioUseResult openUse = client.useItemOn(furnaceUseTarget, client.selectedItem());
        boolean opened = client.waitForScreenClassName(FURNACE_SCREEN, INVENTORY_TIMEOUT);
        observations.add(
            "furnace reopen for cooked food: " + (opened ? "passed" : "failed")
                + " open_use_result=" + openUse.result()
                + " screen=" + FURNACE_SCREEN
                + " screen_matched=" + opened
        );
        if (!opened) {
            return new ClientScenarioReport("failed", id, observations);
        }

        boolean inputMoved = client.moveSelectedItemToContainerSlot(
            0,
            drop.dropItemId(),
            1,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "cooked passive food input transfer: " + (inputMoved ? "passed" : "failed")
                + " slot=0 item=" + drop.dropItemId()
        );
        if (!inputMoved) {
            return new ClientScenarioReport("failed", id, observations);
        }

        boolean fuelMoved = client.moveSelectedItemToContainerSlot(
            1,
            "minecraft:charcoal",
            1,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "cooked passive food fuel transfer: " + (fuelMoved ? "passed" : "failed")
                + " slot=1 item=minecraft:charcoal"
        );
        if (!fuelMoved) {
            return new ClientScenarioReport("failed", id, observations);
        }

        boolean outputReady = client.waitForContainerSlot(2, cookedItemId, 1, FURNACE_COOK_TIMEOUT);
        observations.add(
            "cooked passive food output: " + (outputReady ? "passed" : "failed")
                + " slot=2 item=" + cookedItemId
        );
        if (!outputReady) {
            return new ClientScenarioReport("failed", id, observations);
        }

        boolean outputTaken = client.moveContainerSlotToInventory(
            2,
            cookedItemId,
            1,
            INVENTORY_TIMEOUT
        );
        boolean cookedInInventory = client.waitForInventoryCount(cookedItemId, 1, INVENTORY_TIMEOUT);
        boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        observations.add(
            "cooked passive food inventory: "
                + (outputTaken && cookedInInventory && closed ? "passed" : "failed")
                + " output_taken=" + outputTaken
                + " cooked_inventory_matched=" + cookedInInventory
                + " closed=" + closed
        );
        if (!outputTaken || !cookedInInventory || !closed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        boolean hungerDrained = client.drainHungerBySprinting(HUNGER_DRAIN_TIMEOUT);
        observations.add("natural hunger drain for cooked food: " + (hungerDrained ? "passed" : "failed"));
        if (!hungerDrained) {
            return new ClientScenarioReport("failed", id, observations);
        }

        int foodCountBefore = client.inventoryCount(cookedItemId);
        ScenarioHeldItem selectedFood = client.selectHotbarItem(cookedItemId, 1, HOTBAR_TIMEOUT);
        if (!selectedFood.matches(cookedItemId, 1)) {
            observations.add("blocked: cooked passive food exists but is not selectable from hotbar");
            return new ClientScenarioReport("blocked", id, observations);
        }
        ScenarioFoodUseResult eaten = client.eatSelectedFood(cookedItemId, foodCountBefore, FOOD_EAT_TIMEOUT);
        boolean passed = eaten.started()
            && eaten.foodBefore() < 20
            && eaten.foodAfter() > eaten.foodBefore()
            && eaten.itemCountAfter() < eaten.itemCountBefore();
        observations.add(
            "cooked passive food eating: " + (passed ? "passed" : "failed")
                + " item=" + cookedItemId
                + " raw_item=" + drop.dropItemId()
                + " started=" + eaten.started()
                + " food_before=" + eaten.foodBefore()
                + " food_after=" + eaten.foodAfter()
                + " item_count_before=" + eaten.itemCountBefore()
                + " item_count_after=" + eaten.itemCountAfter()
        );
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runEarnedChestStorage(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        return storeEarnedItemInChest(id, observations, client, true).report();
    }

    private ClientScenarioReport runEarnedDoorPlaceToggle(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        LogToPlanksResult planks = runLogToPlanks(id, observations, client, 3, true, false);
        if (!"passed".equals(planks.report().result())) {
            return planks.report();
        }
        CraftingTableOpenResult table = craftPlaceAndOpenTable(
            id,
            observations,
            client,
            planks.planks().planksItemId(),
            false
        );
        if (!"passed".equals(table.report().result())) {
            return table.report();
        }

        DoorRecipe door = DOOR_BY_PLANKS.get(planks.planks().planksItemId());
        if (door == null) {
            observations.add("blocked: no embedded door recipe mapping for planks=" + planks.planks().planksItemId());
            return new ClientScenarioReport("blocked", id, observations);
        }
        int planksCount = client.inventoryCount(planks.planks().planksItemId());
        int doorCount = client.inventoryCount(door.doorItemId());
        if (planksCount < 6) {
            observations.add("door recipe: failed fewer than six matching planks available");
            return new ClientScenarioReport("failed", id, observations);
        }
        int expectedPlanksAfterDoor = planksCount - 6;
        int expectedDoorCount = doorCount + 3;
        int containerId = client.activeContainerId();
        client.placeRecipe(containerId, door.recipeDisplayId(), false);
        boolean planksConsumed = client.waitForInventoryCount(
            planks.planks().planksItemId(),
            expectedPlanksAfterDoor,
            INVENTORY_TIMEOUT
        );
        boolean doorCreated = client.waitForInventoryCount(
            door.doorItemId(),
            expectedDoorCount,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "door recipe: " + (planksConsumed && doorCreated ? "passed" : "failed")
                + " container_id=" + containerId
                + " recipe_display_id=" + door.recipeDisplayId()
                + " planks_item=" + planks.planks().planksItemId()
                + " planks_expected_count=" + expectedPlanksAfterDoor
                + " planks_count_matched=" + planksConsumed
                + " door_item=" + door.doorItemId()
                + " door_expected_count=" + expectedDoorCount
                + " door_count_matched=" + doorCreated
        );
        if (!planksConsumed || !doorCreated) {
            return new ClientScenarioReport("failed", id, observations);
        }
        boolean craftingClosed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        observations.add("crafting table screen close after door: " + (craftingClosed ? "passed" : "failed"));
        if (!craftingClosed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioHeldItem heldDoor = client.selectHotbarItem(door.doorItemId(), 1, HOTBAR_TIMEOUT);
        if (!heldDoor.matches(door.doorItemId(), 1)) {
            observations.add("blocked: crafted door exists but is not selectable from hotbar");
            return new ClientScenarioReport("blocked", id, observations);
        }
        ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (pair == null) {
            observations.add("blocked: no loaded dry target found for door placement");
            return new ClientScenarioReport("blocked", id, observations);
        }
        ScenarioBlockTarget clicked = new ScenarioBlockTarget(
            pair.clicked().x(),
            pair.clicked().y(),
            pair.clicked().z(),
            pair.clicked().face(),
            "door-clicked",
            pair.clicked().blockId()
        );
        ScenarioBlockTarget doorTarget = new ScenarioBlockTarget(
            pair.target().x(),
            pair.target().y(),
            pair.target().z(),
            "up",
            "door-target",
            door.doorItemId()
        );
        ScenarioUseResult placeUse = client.useItemOn(clicked, heldDoor);
        boolean placed = client.waitForBlock(doorTarget, door.doorItemId(), BLOCK_TIMEOUT);
        observations.add(
            "door placement: " + (placed ? "passed" : "failed")
                + " place_use_result=" + placeUse.result()
                + " target=" + coordinates(doorTarget)
                + " door_item=" + door.doorItemId()
        );
        if (!placed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioUseResult openUse = client.useItemOn(doorTarget, heldDoor);
        boolean openObserved = client.waitForBlock(doorTarget, door.doorItemId(), BLOCK_TIMEOUT);
        observations.add(
            "door toggle open: " + (openObserved ? "passed" : "failed")
                + " open_use_result=" + openUse.result()
                + " door_item=" + door.doorItemId()
        );
        if (!openObserved) {
            return new ClientScenarioReport("failed", id, observations);
        }
        ScenarioUseResult closeUse = client.useItemOn(doorTarget, heldDoor);
        boolean closeObserved = client.waitForBlock(doorTarget, door.doorItemId(), BLOCK_TIMEOUT);
        observations.add(
            "door toggle close: " + (closeObserved ? "passed" : "failed")
                + " close_use_result=" + closeUse.result()
                + " door_item=" + door.doorItemId()
        );
        return new ClientScenarioReport(closeObserved ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runEarnedSignPlaceEdit(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        LogToPlanksResult planks = runLogToPlanks(id, observations, client, 3, true, false);
        if (!"passed".equals(planks.report().result())) {
            return planks.report();
        }
        CraftingTableOpenResult table = craftPlaceAndOpenTable(
            id,
            observations,
            client,
            planks.planks().planksItemId(),
            false
        );
        if (!"passed".equals(table.report().result())) {
            return table.report();
        }

        SignRecipe sign = SIGN_BY_PLANKS.get(planks.planks().planksItemId());
        if (sign == null) {
            observations.add("blocked: no embedded sign recipe mapping for planks=" + planks.planks().planksItemId());
            return new ClientScenarioReport("blocked", id, observations);
        }
        int containerId = client.activeContainerId();
        int planksCountBeforeSticks = client.inventoryCount(planks.planks().planksItemId());
        int stickCountBefore = client.inventoryCount("minecraft:stick");
        if (planksCountBeforeSticks < 8) {
            observations.add("sign recipe: failed fewer than eight matching planks available before stick crafting");
            return new ClientScenarioReport("failed", id, observations);
        }
        int expectedPlanksAfterSticks = planksCountBeforeSticks - 2;
        int expectedStickCountAfterSticks = stickCountBefore + 4;
        client.placeRecipe(containerId, STICK_RECIPE_DISPLAY_ID, false);
        boolean stickPlanksConsumed = client.waitForInventoryCount(
            planks.planks().planksItemId(),
            expectedPlanksAfterSticks,
            INVENTORY_TIMEOUT
        );
        boolean sticksCreated = client.waitForInventoryCount(
            "minecraft:stick",
            expectedStickCountAfterSticks,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "sign stick recipe: " + (stickPlanksConsumed && sticksCreated ? "passed" : "failed")
                + " container_id=" + containerId
                + " recipe_display_id=" + STICK_RECIPE_DISPLAY_ID
                + " planks_item=" + planks.planks().planksItemId()
                + " planks_expected_count=" + expectedPlanksAfterSticks
                + " planks_count_matched=" + stickPlanksConsumed
                + " stick_expected_count=" + expectedStickCountAfterSticks
                + " stick_count_matched=" + sticksCreated
        );
        if (!stickPlanksConsumed || !sticksCreated) {
            return new ClientScenarioReport("failed", id, observations);
        }

        int planksCount = client.inventoryCount(planks.planks().planksItemId());
        int stickCount = client.inventoryCount("minecraft:stick");
        int signCount = client.inventoryCount(sign.signItemId());
        if (planksCount < 6 || stickCount < 1) {
            observations.add("sign recipe: failed missing earned planks or stick");
            return new ClientScenarioReport("failed", id, observations);
        }
        int expectedPlanksAfterSign = planksCount - 6;
        int expectedStickCountAfterSign = stickCount - 1;
        int expectedSignCount = signCount + 3;
        client.placeRecipe(containerId, sign.recipeDisplayId(), false);
        boolean planksConsumed = client.waitForInventoryCount(
            planks.planks().planksItemId(),
            expectedPlanksAfterSign,
            INVENTORY_TIMEOUT
        );
        boolean stickConsumed = client.waitForInventoryCount(
            "minecraft:stick",
            expectedStickCountAfterSign,
            INVENTORY_TIMEOUT
        );
        boolean signCreated = client.waitForInventoryCount(
            sign.signItemId(),
            expectedSignCount,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "sign recipe: " + (planksConsumed && stickConsumed && signCreated ? "passed" : "failed")
                + " container_id=" + containerId
                + " recipe_display_id=" + sign.recipeDisplayId()
                + " planks_item=" + planks.planks().planksItemId()
                + " planks_expected_count=" + expectedPlanksAfterSign
                + " planks_count_matched=" + planksConsumed
                + " stick_expected_count=" + expectedStickCountAfterSign
                + " stick_count_matched=" + stickConsumed
                + " sign_item=" + sign.signItemId()
                + " sign_expected_count=" + expectedSignCount
                + " sign_count_matched=" + signCreated
        );
        if (!planksConsumed || !stickConsumed || !signCreated) {
            return new ClientScenarioReport("failed", id, observations);
        }
        boolean craftingClosed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        observations.add("crafting table screen close after sign: " + (craftingClosed ? "passed" : "failed"));
        if (!craftingClosed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioHeldItem heldSign = client.selectHotbarItem(sign.signItemId(), 1, HOTBAR_TIMEOUT);
        if (!heldSign.matches(sign.signItemId(), 1)) {
            observations.add("blocked: crafted sign exists but is not selectable from hotbar");
            return new ClientScenarioReport("blocked", id, observations);
        }
        ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (pair == null) {
            observations.add("blocked: no loaded dry target found for sign placement");
            return new ClientScenarioReport("blocked", id, observations);
        }
        ScenarioBlockTarget clicked = new ScenarioBlockTarget(
            pair.clicked().x(),
            pair.clicked().y(),
            pair.clicked().z(),
            pair.clicked().face(),
            "sign-clicked",
            pair.clicked().blockId()
        );
        ScenarioBlockTarget signTarget = new ScenarioBlockTarget(
            pair.target().x(),
            pair.target().y(),
            pair.target().z(),
            "up",
            "sign-target",
            sign.signItemId()
        );
        ScenarioUseResult placeUse = client.useItemOn(clicked, heldSign);
        boolean placed = client.waitForBlock(signTarget, sign.signItemId(), BLOCK_TIMEOUT);
        observations.add(
            "sign placement: " + (placed ? "passed" : "failed")
                + " place_use_result=" + placeUse.result()
                + " target=" + coordinates(signTarget)
                + " sign_item=" + sign.signItemId()
        );
        if (!placed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        boolean editorOpen = client.waitForSignEditor(signTarget, INVENTORY_TIMEOUT);
        observations.add("sign editor: " + (editorOpen ? "passed" : "failed"));
        if (!editorOpen) {
            return new ClientScenarioReport("failed", id, observations);
        }
        client.updateSignText(signTarget, P17_SIGN_LINES);
        boolean textVisible = client.waitForSignText(signTarget, P17_SIGN_LINES, INVENTORY_TIMEOUT);
        boolean editorClosed = textVisible && client.closeCurrentScreen(INVENTORY_TIMEOUT);
        observations.add(
            "sign text update: " + (textVisible && editorClosed ? "passed" : "failed")
                + " text_visible=" + textVisible
                + " editor_closed=" + editorClosed
                + " lines=" + String.join("|", P17_SIGN_LINES)
        );
        return new ClientScenarioReport(textVisible && editorClosed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runEarnedCampfireCooking(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        return runEarnedCampfireCooking(id, observations, client, true);
    }

    private ClientScenarioReport runEarnedCampfireCooking(
        String id,
        List<String> observations,
        ScenarioClient client,
        boolean requireCookedPickup
    ) throws Exception {
        return prepareEarnedCampfireCooking(id, observations, client, requireCookedPickup).report();
    }

    private CampfireCookingResult prepareEarnedCampfireCooking(
        String id,
        List<String> observations,
        ScenarioClient client,
        boolean requireCookedPickup
    ) throws Exception {
        WoodenToolTableResult prepared = prepareWoodenToolAndTable(id, observations, client);
        if (!"passed".equals(prepared.report().result())) {
            return new CampfireCookingResult(prepared.report(), null);
        }

        for (int log = 0; log < 4; log++) {
            ClientScenarioReport collectedLog = collectNaturalLogItem(
                id,
                observations,
                client,
                prepared.planks(),
                "campfire reserve log"
            );
            if (!"passed".equals(collectedLog.result())) {
                return new CampfireCookingResult(collectedLog, null);
            }
        }

        PassiveFoodDropResult drop = collectPassiveFoodDrop(id, observations, client);
        if (!"passed".equals(drop.report().result())) {
            return new CampfireCookingResult(drop.report(), null);
        }
        String cookedItemId = COOKED_PASSIVE_FOOD_RESULTS.get(drop.dropItemId());
        if (cookedItemId == null) {
            observations.add("blocked: passive food drop has no campfire cooking result item=" + drop.dropItemId());
            return new CampfireCookingResult(new ClientScenarioReport("blocked", id, observations), null);
        }

        FurnacePlacementOpenResult furnace = finishFurnacePlacementOpen(
            id,
            observations,
            client,
            prepared.planks(),
            prepared.tableTarget()
        );
        if (!"passed".equals(furnace.report().result())) {
            return new CampfireCookingResult(furnace.report(), null);
        }

        FurnaceCharcoalSmeltResult charcoal = smeltCharcoalInPlacedFurnace(id, observations, client, furnace);
        if (!"passed".equals(charcoal.report().result())) {
            return new CampfireCookingResult(charcoal.report(), null);
        }

        ScenarioBlockTarget craftingTableTarget = prepared.tableTarget();
        boolean tableApproached = client.approachBlock(craftingTableTarget, APPROACH_TIMEOUT);
        observations.add(
            "crafting table approach for campfire: " + (tableApproached ? "passed" : "failed")
                + " target=" + coordinates(craftingTableTarget)
        );
        if (!tableApproached) {
            CraftingTableOpenResult spareTable = craftPlaceAndOpenTable(
                id,
                observations,
                client,
                prepared.planks().planksItemId(),
                false
            );
            if (!"passed".equals(spareTable.report().result())) {
                return new CampfireCookingResult(spareTable.report(), null);
            }
            craftingTableTarget = spareTable.tableTarget();
        }
        if (tableApproached) {
            ScenarioBlockTarget tableUseTarget = reachableUseTarget(client, craftingTableTarget);
            ScenarioUseResult tableOpenUse = client.useItemOn(tableUseTarget, client.selectedItem());
            boolean tableOpened = client.waitForScreenClassName(CRAFTING_SCREEN, INVENTORY_TIMEOUT);
            observations.add(
                "crafting table reopen for campfire: " + (tableOpened ? "passed" : "failed")
                    + " open_use_result=" + tableOpenUse.result()
                    + " screen=" + CRAFTING_SCREEN
                    + " screen_matched=" + tableOpened
            );
            if (!tableOpened) {
                return new CampfireCookingResult(new ClientScenarioReport("failed", id, observations), null);
            }
        }

        int containerId = client.activeContainerId();
        int stickCount = client.inventoryCount("minecraft:stick");
        if (stickCount < 3) {
            int planksCount = client.inventoryCount(prepared.planks().planksItemId());
            if (planksCount < 2) {
                observations.add("campfire stick recipe: failed fewer than two planks available");
                return new CampfireCookingResult(new ClientScenarioReport("failed", id, observations), null);
            }
            int expectedPlanksAfterSticks = planksCount - 2;
            int expectedStickCount = stickCount + 4;
            client.placeRecipe(containerId, STICK_RECIPE_DISPLAY_ID, false);
            boolean planksConsumed = client.waitForInventoryCount(
                prepared.planks().planksItemId(),
                expectedPlanksAfterSticks,
                INVENTORY_TIMEOUT
            );
            boolean sticksCreated = client.waitForInventoryCount(
                "minecraft:stick",
                expectedStickCount,
                INVENTORY_TIMEOUT
            );
            observations.add(
                "campfire stick recipe: " + (planksConsumed && sticksCreated ? "passed" : "failed")
                    + " container_id=" + containerId
                    + " recipe_display_id=" + STICK_RECIPE_DISPLAY_ID
                    + " planks_item=" + prepared.planks().planksItemId()
                    + " planks_expected_count=" + expectedPlanksAfterSticks
                    + " planks_count_matched=" + planksConsumed
                    + " stick_expected_count=" + expectedStickCount
                    + " stick_count_matched=" + sticksCreated
            );
            if (!planksConsumed || !sticksCreated) {
                return new CampfireCookingResult(new ClientScenarioReport("failed", id, observations), null);
            }
        }

        int logCount = client.inventoryCount(prepared.planks().logItemId());
        stickCount = client.inventoryCount("minecraft:stick");
        int charcoalCount = client.inventoryCount("minecraft:charcoal");
        int campfireCount = client.inventoryCount("minecraft:campfire");
        if (logCount < 3 || stickCount < 3 || charcoalCount < 1) {
            observations.add(
                "campfire recipe: failed missing earned logs, sticks, or charcoal"
                    + " log_count=" + logCount
                    + " stick_count=" + stickCount
                    + " charcoal_count=" + charcoalCount
            );
            return new CampfireCookingResult(new ClientScenarioReport("failed", id, observations), null);
        }
        int expectedLogCount = logCount - 3;
        int expectedStickCount = stickCount - 3;
        int expectedCharcoalCount = charcoalCount - 1;
        int expectedCampfireCount = campfireCount + 1;
        client.placeRecipe(containerId, CAMPFIRE_RECIPE_DISPLAY_ID, false);
        boolean logsConsumed = client.waitForInventoryCount(
            prepared.planks().logItemId(),
            expectedLogCount,
            INVENTORY_TIMEOUT
        );
        boolean sticksConsumed = client.waitForInventoryCount(
            "minecraft:stick",
            expectedStickCount,
            INVENTORY_TIMEOUT
        );
        boolean charcoalConsumed = client.waitForInventoryCount(
            "minecraft:charcoal",
            expectedCharcoalCount,
            INVENTORY_TIMEOUT
        );
        boolean campfireCreated = client.waitForInventoryCount(
            "minecraft:campfire",
            expectedCampfireCount,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "campfire recipe: "
                + (logsConsumed && sticksConsumed && charcoalConsumed && campfireCreated ? "passed" : "failed")
                + " container_id=" + containerId
                + " recipe_display_id=" + CAMPFIRE_RECIPE_DISPLAY_ID
                + " log_item=" + prepared.planks().logItemId()
                + " log_expected_count=" + expectedLogCount
                + " log_count_matched=" + logsConsumed
                + " stick_expected_count=" + expectedStickCount
                + " stick_count_matched=" + sticksConsumed
                + " charcoal_expected_count=" + expectedCharcoalCount
                + " charcoal_count_matched=" + charcoalConsumed
                + " campfire_expected_count=" + expectedCampfireCount
                + " campfire_count_matched=" + campfireCreated
        );
        if (!logsConsumed || !sticksConsumed || !charcoalConsumed || !campfireCreated) {
            return new CampfireCookingResult(new ClientScenarioReport("failed", id, observations), null);
        }
        boolean craftingClosed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        observations.add("crafting table screen close after campfire: " + (craftingClosed ? "passed" : "failed"));
        if (!craftingClosed) {
            return new CampfireCookingResult(new ClientScenarioReport("failed", id, observations), null);
        }

        ScenarioHeldItem heldCampfire = client.selectHotbarItem("minecraft:campfire", 1, HOTBAR_TIMEOUT);
        if (!heldCampfire.matches("minecraft:campfire", 1)) {
            observations.add("blocked: crafted campfire exists but is not selectable from hotbar");
            return new CampfireCookingResult(new ClientScenarioReport("blocked", id, observations), null);
        }
        ScenarioBlockPair pair = client.findOpenDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (pair == null) {
            observations.add("blocked: no loaded open dry target found for campfire placement");
            return new CampfireCookingResult(new ClientScenarioReport("blocked", id, observations), null);
        }
        ScenarioBlockTarget clicked = new ScenarioBlockTarget(
            pair.clicked().x(),
            pair.clicked().y(),
            pair.clicked().z(),
            pair.clicked().face(),
            "campfire-clicked",
            pair.clicked().blockId()
        );
        ScenarioBlockTarget campfireTarget = new ScenarioBlockTarget(
            pair.target().x(),
            pair.target().y(),
            pair.target().z(),
            "up",
            "campfire-target",
            "minecraft:campfire"
        );
        ScenarioUseResult placeUse = client.useItemOn(clicked, heldCampfire);
        boolean placed = client.waitForBlock(campfireTarget, "minecraft:campfire", BLOCK_TIMEOUT);
        observations.add(
            "campfire placement: " + (placed ? "passed" : "failed")
                + " place_use_result=" + placeUse.result()
                + " target=" + coordinates(campfireTarget)
        );
        if (!placed) {
            return new CampfireCookingResult(new ClientScenarioReport("failed", id, observations), null);
        }

        int cookedCountBefore = client.inventoryCount(cookedItemId);
        ScenarioHeldItem rawFood = client.selectHotbarItem(drop.dropItemId(), 1, HOTBAR_TIMEOUT);
        if (!rawFood.matches(drop.dropItemId(), 1)) {
            observations.add("blocked: earned raw passive food exists but is not selectable from hotbar");
            return new CampfireCookingResult(new ClientScenarioReport("blocked", id, observations), campfireTarget);
        }
        ScenarioUseResult cookUse = client.useItemOn(campfireTarget, rawFood);
        boolean outputVisible = client.waitForVisibleItemDrop(cookedItemId, campfireTarget, CAMPFIRE_COOK_TIMEOUT);
        ScenarioBreakResult pickup = outputVisible
            ? client.collectVisibleItemDrop(campfireTarget, cookedItemId, 1, PICKUP_TIMEOUT)
            : new ScenarioBreakResult(false, false, false, false, rawFood);
        boolean cookedCollected = outputVisible
            && pickup.pickupRestored()
            && client.waitForInventoryCount(cookedItemId, cookedCountBefore + 1, INVENTORY_TIMEOUT);
        boolean cookingSatisfied = requireCookedPickup ? cookedCollected : outputVisible;
        observations.add(
            "campfire cooking output: " + (cookingSatisfied ? "passed" : "failed")
                + " cook_use_result=" + cookUse.result()
                + " raw_item=" + drop.dropItemId()
                + " cooked_item=" + cookedItemId
                + " output_visible=" + outputVisible
                + " pickup_restored=" + pickup.pickupRestored()
                + " pickup_required=" + requireCookedPickup
                + " cooked_expected_count=" + (cookedCountBefore + 1)
        );
        return new CampfireCookingResult(
            new ClientScenarioReport(cookingSatisfied ? "passed" : "failed", id, observations),
            campfireTarget
        );
    }

    private ClientScenarioReport runEarnedCampfireDeathRespawn(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        return performEarnedCampfireDeathRespawn(id, observations, client, false).report();
    }

    private ClientScenarioReport runCampfireDeathDropRecovery(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        CampfireDeathRespawnResult death = performEarnedCampfireDeathRespawn(id, observations, client, true);
        if (!"passed".equals(death.report().result())) {
            return death.report();
        }

        int pickaxeCountBeforeRecovery = client.inventoryCount("minecraft:wooden_pickaxe");
        boolean deathSiteApproached = client.approachBlock(death.campfireTarget(), APPROACH_TIMEOUT);
        observations.add(
            "campfire death-site return: " + (deathSiteApproached ? "passed" : "failed")
                + " target=" + coordinates(death.campfireTarget())
        );
        if (!deathSiteApproached) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioBreakResult pickup = client.collectVisibleItemDropByIdentity(
            death.campfireTarget(),
            "minecraft:wooden_pickaxe",
            death.woodenPickaxeDropIdentity(),
            1,
            PICKUP_TIMEOUT
        );
        boolean pickupVisible = pickup.sawDrop();
        boolean pickupDisappeared = pickup.becameAir();
        boolean inventoryExact = pickupVisible
            && pickupDisappeared
            && pickup.pickupRestored()
            && client.waitForInventoryCount(
                "minecraft:wooden_pickaxe",
                pickaxeCountBeforeRecovery + 1,
                INVENTORY_TIMEOUT
            );
        observations.add(
            "campfire death-drop recovery: " + (inventoryExact ? "passed" : "failed")
                + " item=minecraft:wooden_pickaxe"
                + " identity=" + death.woodenPickaxeDropIdentity()
                + " pickup_visible=" + pickupVisible
                + " pickup_disappeared=" + pickupDisappeared
                + " pickup_restored=" + pickup.pickupRestored()
                + " inventory_exact=" + inventoryExact
                + " expected_count=" + (pickaxeCountBeforeRecovery + 1)
        );
        return new ClientScenarioReport(inventoryExact ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runEarnedToolZombieCombat(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        WoodenToolTableResult prepared = prepareWoodenToolAndTable(id, observations, client);
        if (!"passed".equals(prepared.report().result())) {
            return prepared.report();
        }
        boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        observations.add("crafting table screen close before zombie combat: " + (closed ? "passed" : "failed"));
        if (!closed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioHeldItem weapon = client.selectHotbarItem("minecraft:wooden_pickaxe", 1, HOTBAR_TIMEOUT);
        if (!weapon.matches("minecraft:wooden_pickaxe", 1)) {
            observations.add("blocked: earned wooden pickaxe exists but is not selectable for zombie combat");
            return new ClientScenarioReport("blocked", id, observations);
        }

        boolean nightReached = client.waitForDayTimeAtOrAfter(NIGHT_START_DAY_TIME, NIGHT_WAIT_TIMEOUT);
        observations.add(
            "natural night wait for zombie combat: " + (nightReached ? "passed" : "failed")
                + " night_start_day_time=" + NIGHT_START_DAY_TIME
        );
        if (!nightReached) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioEntityObservation zombie = client.findVisibleEntity(
            ZOMBIE_ENTITY_IDS,
            ScenarioReach.OUTSIDE_SURVIVAL_REACH,
            ENTITY_SCAN_TIMEOUT
        );
        if (zombie == null) {
            zombie = client.findVisibleEntity(
                ZOMBIE_ENTITY_IDS,
                ScenarioReach.WITHIN_SURVIVAL_REACH,
                ENTITY_SCAN_TIMEOUT
            );
        }
        if (zombie == null) {
            observations.add("blocked: no loaded natural zombie visible after nightfall");
            return new ClientScenarioReport("blocked", id, observations);
        }
        observations.add(
            "zombie scan: passed"
                + " entity=" + zombie.entityType()
                + " entity_id=" + zombie.entityId()
                + " distance_squared=" + zombie.distanceSquared()
        );

        boolean approached = client.approachEntity(zombie, APPROACH_TIMEOUT);
        observations.add(
            "zombie approach: " + (approached ? "passed" : "failed")
                + " entity_id=" + zombie.entityId()
        );
        if (!approached) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        int rottenFleshBefore = client.inventoryCount("minecraft:rotten_flesh");
        ScenarioBreakResult attack = client.attackEntityUntilDropCollected(
            zombie,
            "minecraft:rotten_flesh",
            1,
            ENTITY_ATTACK_TIMEOUT
        );
        int rottenFleshAfter = client.inventoryCount("minecraft:rotten_flesh");
        boolean collected = attack.started()
            && attack.becameAir()
            && attack.pickupRestored()
            && rottenFleshAfter >= rottenFleshBefore + 1;
        observations.add(
            "zombie combat drop: " + (collected ? "passed" : "failed")
                + " entity_id=" + zombie.entityId()
                + " weapon=" + weapon.itemId()
                + " expected_drop=minecraft:rotten_flesh"
                + " attack_started=" + attack.started()
                + " entity_removed=" + attack.becameAir()
                + " saw_drop=" + attack.sawDrop()
                + " pickup_restored=" + attack.pickupRestored()
                + " rotten_flesh_before=" + rottenFleshBefore
                + " rotten_flesh_after=" + rottenFleshAfter
        );
        float healthAfterCombat = client.playerHealth();
        boolean survived = healthAfterCombat > 0.0F;
        observations.add(
            "zombie combat survival: " + (survived ? "passed" : "failed")
                + " health_after=" + healthAfterCombat
        );
        return new ClientScenarioReport(collected && survived ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runStoneSwordZombieCombat(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        WoodenToolTableResult prepared = prepareWoodenToolAndTable(id, observations, client);
        if (!"passed".equals(prepared.report().result())) {
            return prepared.report();
        }

        ClientScenarioReport cobblestone = mineCobblestoneWithWoodenPickaxe(id, observations, client, 2);
        if (!"passed".equals(cobblestone.result())) {
            return cobblestone;
        }

        boolean tableApproached = client.approachBlock(prepared.tableTarget(), APPROACH_TIMEOUT);
        observations.add(
            "crafting table approach for stone sword: " + (tableApproached ? "passed" : "failed")
                + " target=" + coordinates(prepared.tableTarget())
        );
        if (!tableApproached) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        ScenarioHeldItem woodenPickaxe = client.selectHotbarItem("minecraft:wooden_pickaxe", 1, HOTBAR_TIMEOUT);
        ScenarioBlockTarget tableUseTarget = reachableUseTarget(client, prepared.tableTarget());
        ScenarioUseResult openUse = client.useItemOn(tableUseTarget, woodenPickaxe);
        boolean opened = client.waitForScreenClassName(CRAFTING_SCREEN, INVENTORY_TIMEOUT);
        observations.add(
            "crafting table reopen for stone sword: " + (opened ? "passed" : "failed")
                + " open_use_result=" + openUse.result()
                + " screen=" + CRAFTING_SCREEN
                + " screen_matched=" + opened
        );
        if (!opened) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ClientScenarioReport stoneSword = craftStoneSwordInOpenTable(id, observations, client, true);
        if (!"passed".equals(stoneSword.result())) {
            return stoneSword;
        }

        ScenarioHeldItem weapon = client.selectHotbarItem("minecraft:stone_sword", 1, HOTBAR_TIMEOUT);
        if (!weapon.matches("minecraft:stone_sword", 1)) {
            observations.add("blocked: earned stone sword exists but is not selectable for zombie combat");
            return new ClientScenarioReport("blocked", id, observations);
        }

        boolean nightReached = client.waitForDayTimeAtOrAfter(NIGHT_START_DAY_TIME, NIGHT_WAIT_TIMEOUT);
        observations.add(
            "natural night wait for stone sword zombie combat: " + (nightReached ? "passed" : "failed")
                + " night_start_day_time=" + NIGHT_START_DAY_TIME
        );
        if (!nightReached) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioEntityObservation zombie = client.findVisibleEntity(
            ZOMBIE_ENTITY_IDS,
            ScenarioReach.OUTSIDE_SURVIVAL_REACH,
            ENTITY_SCAN_TIMEOUT
        );
        if (zombie == null) {
            zombie = client.findVisibleEntity(
                ZOMBIE_ENTITY_IDS,
                ScenarioReach.WITHIN_SURVIVAL_REACH,
                ENTITY_SCAN_TIMEOUT
            );
        }
        if (zombie == null) {
            observations.add("blocked: no loaded natural zombie visible after nightfall");
            return new ClientScenarioReport("blocked", id, observations);
        }
        observations.add(
            "stone sword zombie scan: passed"
                + " entity=" + zombie.entityType()
                + " entity_id=" + zombie.entityId()
                + " distance_squared=" + zombie.distanceSquared()
        );

        boolean approached = client.approachEntity(zombie, APPROACH_TIMEOUT);
        observations.add(
            "stone sword zombie approach: " + (approached ? "passed" : "failed")
                + " entity_id=" + zombie.entityId()
        );
        if (!approached) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        int rottenFleshBefore = client.inventoryCount("minecraft:rotten_flesh");
        ScenarioBreakResult attack = client.attackEntityUntilDropCollected(
            zombie,
            "minecraft:rotten_flesh",
            1,
            ENTITY_ATTACK_TIMEOUT
        );
        int rottenFleshAfter = client.inventoryCount("minecraft:rotten_flesh");
        boolean collected = attack.started()
            && attack.becameAir()
            && attack.pickupRestored()
            && rottenFleshAfter >= rottenFleshBefore + 1;
        observations.add(
            "stone sword zombie combat: " + (collected ? "passed" : "failed")
                + " entity_id=" + zombie.entityId()
                + " weapon=" + weapon.itemId()
                + " expected_drop=minecraft:rotten_flesh"
                + " attack_started=" + attack.started()
                + " entity_removed=" + attack.becameAir()
                + " saw_drop=" + attack.sawDrop()
                + " pickup_restored=" + attack.pickupRestored()
                + " rotten_flesh_before=" + rottenFleshBefore
                + " rotten_flesh_after=" + rottenFleshAfter
        );
        float healthAfterCombat = client.playerHealth();
        boolean survived = healthAfterCombat > 0.0F;
        observations.add(
            "stone sword zombie combat survival: " + (survived ? "passed" : "failed")
                + " health_after=" + healthAfterCombat
        );
        return new ClientScenarioReport(collected && survived ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runIronIngotProgression(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        IronProgressionBaseResult base = prepareIronProgressionBase(id, observations, client);
        if (!"passed".equals(base.report().result())) {
            return base.report();
        }

        for (int ore = 0; ore < 2; ore++) {
            ClientScenarioReport rawIron = mineRawIronWithStonePickaxe(id, observations, client);
            if (!"passed".equals(rawIron.result())) {
                return rawIron;
            }
        }

        return smeltRawIronInPlacedFurnace(
            id,
            observations,
            client,
            base.placedFurnace(),
            base.prepared().planks(),
            2
        );
    }

    private IronProgressionBaseResult prepareIronProgressionBase(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        WoodenToolTableResult prepared = prepareWoodenToolAndTable(id, observations, client);
        if (!"passed".equals(prepared.report().result())) {
            return new IronProgressionBaseResult(prepared.report(), prepared, null);
        }

        ClientScenarioReport cobblestone = mineCobblestoneWithWoodenPickaxe(id, observations, client, 11);
        if (!"passed".equals(cobblestone.result())) {
            return new IronProgressionBaseResult(cobblestone, prepared, null);
        }

        boolean tableApproached = client.approachBlock(prepared.tableTarget(), APPROACH_TIMEOUT);
        observations.add(
            "crafting table approach for iron progression: " + (tableApproached ? "passed" : "failed")
                + " target=" + coordinates(prepared.tableTarget())
        );
        if (!tableApproached) {
            return new IronProgressionBaseResult(
                new ClientScenarioReport("blocked", id, observations),
                prepared,
                null
            );
        }

        ScenarioHeldItem woodenPickaxe = client.selectHotbarItem("minecraft:wooden_pickaxe", 1, HOTBAR_TIMEOUT);
        ScenarioBlockTarget tableUseTarget = reachableUseTarget(client, prepared.tableTarget());
        ScenarioUseResult openUse = client.useItemOn(tableUseTarget, woodenPickaxe);
        boolean opened = client.waitForScreenClassName(CRAFTING_SCREEN, INVENTORY_TIMEOUT);
        observations.add(
            "crafting table reopen for iron progression: " + (opened ? "passed" : "failed")
                + " open_use_result=" + openUse.result()
                + " screen=" + CRAFTING_SCREEN
                + " screen_matched=" + opened
        );
        if (!opened) {
            return new IronProgressionBaseResult(
                new ClientScenarioReport("failed", id, observations),
                prepared,
                null
            );
        }

        ClientScenarioReport stonePickaxe = craftStonePickaxeInOpenTable(id, observations, client, false);
        if (!"passed".equals(stonePickaxe.result())) {
            return new IronProgressionBaseResult(stonePickaxe, prepared, null);
        }
        ClientScenarioReport furnace = craftFurnaceInOpenTable(id, observations, client, true);
        if (!"passed".equals(furnace.result())) {
            return new IronProgressionBaseResult(furnace, prepared, null);
        }

        FurnacePlacementOpenResult placedFurnace = placeAndOpenFurnace(id, observations, client);
        if (!"passed".equals(placedFurnace.report().result())) {
            return new IronProgressionBaseResult(placedFurnace.report(), prepared, placedFurnace);
        }

        return new IronProgressionBaseResult(
            new ClientScenarioReport("passed", id, observations),
            prepared,
            placedFurnace
        );
    }

    private IronSwordProgressionResult craftEarnedIronSwordProgression(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        IronProgressionBaseResult base = prepareIronProgressionBase(id, observations, client);
        if (!"passed".equals(base.report().result())) {
            return new IronSwordProgressionResult(base.report(), null);
        }

        for (int ingot = 0; ingot < 2; ingot++) {
            ClientScenarioReport rawIron = mineRawIronWithStonePickaxe(id, observations, client);
            if (!"passed".equals(rawIron.result())) {
                return new IronSwordProgressionResult(rawIron, base.prepared().tableTarget());
            }
            ClientScenarioReport smelt = smeltRawIronInPlacedFurnace(
                id,
                observations,
                client,
                base.placedFurnace(),
                base.prepared().planks()
            );
            if (!"passed".equals(smelt.result())) {
                return new IronSwordProgressionResult(smelt, base.prepared().tableTarget());
            }
        }

        boolean tableApproached = client.approachBlock(base.prepared().tableTarget(), APPROACH_TIMEOUT);
        observations.add(
            "crafting table approach for iron sword: " + (tableApproached ? "passed" : "failed")
                + " target=" + coordinates(base.prepared().tableTarget())
        );
        if (!tableApproached) {
            return new IronSwordProgressionResult(
                new ClientScenarioReport("blocked", id, observations),
                base.prepared().tableTarget()
            );
        }

        ScenarioHeldItem stonePickaxe = client.selectHotbarItem("minecraft:stone_pickaxe", 1, HOTBAR_TIMEOUT);
        ScenarioBlockTarget tableUseTarget = reachableUseTarget(client, base.prepared().tableTarget());
        ScenarioUseResult openUse = client.useItemOn(tableUseTarget, stonePickaxe);
        boolean opened = client.waitForScreenClassName(CRAFTING_SCREEN, INVENTORY_TIMEOUT);
        observations.add(
            "crafting table reopen for iron sword: " + (opened ? "passed" : "failed")
                + " open_use_result=" + openUse.result()
                + " screen=" + CRAFTING_SCREEN
                + " screen_matched=" + opened
        );
        if (!opened) {
            return new IronSwordProgressionResult(
                new ClientScenarioReport("failed", id, observations),
                base.prepared().tableTarget()
            );
        }

        ClientScenarioReport ironSword = craftIronSwordInOpenTable(
            id,
            observations,
            client,
            base.prepared().planks().planksItemId(),
            true
        );
        if (!"passed".equals(ironSword.result())) {
            return new IronSwordProgressionResult(ironSword, base.prepared().tableTarget());
        }

        return new IronSwordProgressionResult(
            new ClientScenarioReport("passed", id, observations),
            base.prepared().tableTarget()
        );
    }

    private ClientScenarioReport craftEarnedShieldProgression(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        IronProgressionBaseResult base = prepareIronProgressionBase(id, observations, client);
        if (!"passed".equals(base.report().result())) {
            return base.report();
        }

        ClientScenarioReport rawIron = mineRawIronWithStonePickaxe(id, observations, client);
        if (!"passed".equals(rawIron.result())) {
            return rawIron;
        }
        ClientScenarioReport smelt = smeltRawIronInPlacedFurnace(
            id,
            observations,
            client,
            base.placedFurnace(),
            base.prepared().planks()
        );
        if (!"passed".equals(smelt.result())) {
            return smelt;
        }

        boolean tableApproached = client.approachBlock(base.prepared().tableTarget(), APPROACH_TIMEOUT);
        observations.add(
            "crafting table approach for shield: " + (tableApproached ? "passed" : "failed")
                + " target=" + coordinates(base.prepared().tableTarget())
        );
        if (!tableApproached) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        ScenarioHeldItem stonePickaxe = client.selectHotbarItem("minecraft:stone_pickaxe", 1, HOTBAR_TIMEOUT);
        ScenarioBlockTarget tableUseTarget = reachableUseTarget(client, base.prepared().tableTarget());
        ScenarioUseResult openUse = client.useItemOn(tableUseTarget, stonePickaxe);
        boolean opened = client.waitForScreenClassName(CRAFTING_SCREEN, INVENTORY_TIMEOUT);
        observations.add(
            "crafting table reopen for shield: " + (opened ? "passed" : "failed")
                + " open_use_result=" + openUse.result()
                + " screen=" + CRAFTING_SCREEN
                + " screen_matched=" + opened
        );
        if (!opened) {
            return new ClientScenarioReport("failed", id, observations);
        }

        return craftShieldInOpenTable(
            id,
            observations,
            client,
            base.prepared().planks().planksItemId(),
            true
        );
    }

    private ClientScenarioReport craftEarnedIronChestplateProgression(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        return craftEarnedIronChestplateProgressionResult(id, observations, client).report();
    }

    private IronChestplateProgressionResult craftEarnedIronChestplateProgressionResult(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        IronProgressionBaseResult base = prepareIronProgressionBase(id, observations, client);
        if (!"passed".equals(base.report().result())) {
            return new IronChestplateProgressionResult(base.report(), null);
        }

        PlanksRecipe fuelPlanks = base.prepared().planks();
        int fuelPlanksCount = client.inventoryCount(fuelPlanks.planksItemId());
        boolean enoughFuel = fuelPlanksCount >= 4;
        observations.add(
            "iron chestplate fuel planks: " + (enoughFuel ? "passed" : "failed")
                + " planks_item=" + fuelPlanks.planksItemId()
                + " planks_count=" + fuelPlanksCount
                + " expected_at_least=4"
        );
        if (!enoughFuel) {
            return new IronChestplateProgressionResult(
                new ClientScenarioReport("failed", id, observations),
                base.prepared().tableTarget()
            );
        }

        for (int ingot = 0; ingot < 8; ingot++) {
            ClientScenarioReport rawIron = mineRawIronWithStonePickaxe(id, observations, client);
            if (!"passed".equals(rawIron.result())) {
                return new IronChestplateProgressionResult(rawIron, base.prepared().tableTarget());
            }
            ClientScenarioReport smelt = smeltRawIronInPlacedFurnace(
                id,
                observations,
                client,
                base.placedFurnace(),
                fuelPlanks,
                ingot % 2 == 0,
                1
            );
            if (!"passed".equals(smelt.result())) {
                return new IronChestplateProgressionResult(smelt, base.prepared().tableTarget());
            }
        }

        boolean tableApproached = client.approachBlock(base.prepared().tableTarget(), APPROACH_TIMEOUT);
        observations.add(
            "crafting table approach for iron chestplate: " + (tableApproached ? "passed" : "failed")
                + " target=" + coordinates(base.prepared().tableTarget())
        );
        if (!tableApproached) {
            return new IronChestplateProgressionResult(
                new ClientScenarioReport("blocked", id, observations),
                base.prepared().tableTarget()
            );
        }

        ScenarioHeldItem stonePickaxe = client.selectHotbarItem("minecraft:stone_pickaxe", 1, HOTBAR_TIMEOUT);
        ScenarioBlockTarget tableUseTarget = reachableUseTarget(client, base.prepared().tableTarget());
        ScenarioUseResult openUse = client.useItemOn(tableUseTarget, stonePickaxe);
        boolean opened = client.waitForScreenClassName(CRAFTING_SCREEN, INVENTORY_TIMEOUT);
        observations.add(
            "crafting table reopen for iron chestplate: " + (opened ? "passed" : "failed")
                + " open_use_result=" + openUse.result()
                + " screen=" + CRAFTING_SCREEN
                + " screen_matched=" + opened
        );
        if (!opened) {
            return new IronChestplateProgressionResult(
                new ClientScenarioReport("failed", id, observations),
                base.prepared().tableTarget()
            );
        }

        return new IronChestplateProgressionResult(
            craftIronChestplateInOpenTable(id, observations, client, true),
            base.prepared().tableTarget()
        );
    }

    private ClientScenarioReport runIronSwordZombieCombat(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        IronSwordProgressionResult ironSword = craftEarnedIronSwordProgression(id, observations, client);
        if (!"passed".equals(ironSword.report().result())) {
            return ironSword.report();
        }

        ScenarioHeldItem weapon = client.selectHotbarItem("minecraft:iron_sword", 1, HOTBAR_TIMEOUT);
        if (!weapon.matches("minecraft:iron_sword", 1)) {
            observations.add("blocked: earned iron sword exists but is not selectable for zombie combat");
            return new ClientScenarioReport("blocked", id, observations);
        }

        boolean nightReached = client.waitForDayTimeAtOrAfter(NIGHT_START_DAY_TIME, NIGHT_WAIT_TIMEOUT);
        observations.add(
            "natural night wait for iron sword zombie combat: " + (nightReached ? "passed" : "failed")
                + " night_start_day_time=" + NIGHT_START_DAY_TIME
        );
        if (!nightReached) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioEntityObservation zombie = client.findVisibleEntity(
            ZOMBIE_ENTITY_IDS,
            ScenarioReach.OUTSIDE_SURVIVAL_REACH,
            ENTITY_SCAN_TIMEOUT
        );
        if (zombie == null) {
            zombie = client.findVisibleEntity(
                ZOMBIE_ENTITY_IDS,
                ScenarioReach.WITHIN_SURVIVAL_REACH,
                ENTITY_SCAN_TIMEOUT
            );
        }
        if (zombie == null) {
            observations.add("blocked: no loaded natural zombie visible after nightfall");
            return new ClientScenarioReport("blocked", id, observations);
        }
        observations.add(
            "iron sword zombie scan: passed"
                + " entity=" + zombie.entityType()
                + " entity_id=" + zombie.entityId()
                + " distance_squared=" + zombie.distanceSquared()
        );

        boolean approached = client.approachEntity(zombie, APPROACH_TIMEOUT);
        observations.add(
            "iron sword zombie approach: " + (approached ? "passed" : "failed")
                + " entity_id=" + zombie.entityId()
        );
        if (!approached) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        int rottenFleshBefore = client.inventoryCount("minecraft:rotten_flesh");
        ScenarioBreakResult attack = client.attackEntityUntilDropCollected(
            zombie,
            "minecraft:rotten_flesh",
            1,
            ENTITY_ATTACK_TIMEOUT
        );
        int rottenFleshAfter = client.inventoryCount("minecraft:rotten_flesh");
        boolean collected = attack.started()
            && attack.becameAir()
            && attack.pickupRestored()
            && rottenFleshAfter >= rottenFleshBefore + 1;
        observations.add(
            "iron sword zombie combat: " + (collected ? "passed" : "failed")
                + " entity_id=" + zombie.entityId()
                + " weapon=" + weapon.itemId()
                + " expected_drop=minecraft:rotten_flesh"
                + " attack_started=" + attack.started()
                + " entity_removed=" + attack.becameAir()
                + " saw_drop=" + attack.sawDrop()
                + " pickup_restored=" + attack.pickupRestored()
                + " rotten_flesh_before=" + rottenFleshBefore
                + " rotten_flesh_after=" + rottenFleshAfter
        );
        float healthAfterCombat = client.playerHealth();
        boolean survived = healthAfterCombat > 0.0F;
        observations.add(
            "iron sword zombie combat survival: " + (survived ? "passed" : "failed")
                + " health_after=" + healthAfterCombat
        );
        return new ClientScenarioReport(collected && survived ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runEarnedShieldZombieBlock(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        ClientScenarioReport shield = craftEarnedShieldProgression(id, observations, client);
        if (!"passed".equals(shield.result())) {
            return shield;
        }

        ScenarioHeldItem heldShield = client.selectHotbarItem("minecraft:shield", 1, HOTBAR_TIMEOUT);
        if (!heldShield.matches("minecraft:shield", 1)) {
            observations.add("blocked: earned shield exists but is not selectable for zombie block");
            return new ClientScenarioReport("blocked", id, observations);
        }

        boolean nightReached = client.waitForDayTimeAtOrAfter(NIGHT_START_DAY_TIME, NIGHT_WAIT_TIMEOUT);
        observations.add(
            "natural night wait for shield zombie block: " + (nightReached ? "passed" : "failed")
                + " night_start_day_time=" + NIGHT_START_DAY_TIME
        );
        if (!nightReached) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioEntityObservation zombie = client.findVisibleEntity(
            ZOMBIE_ENTITY_IDS,
            ScenarioReach.OUTSIDE_SURVIVAL_REACH,
            ENTITY_SCAN_TIMEOUT
        );
        if (zombie == null) {
            zombie = client.findVisibleEntity(
                ZOMBIE_ENTITY_IDS,
                ScenarioReach.WITHIN_SURVIVAL_REACH,
                ENTITY_SCAN_TIMEOUT
            );
        }
        if (zombie == null) {
            observations.add("blocked: no loaded natural zombie visible after nightfall");
            return new ClientScenarioReport("blocked", id, observations);
        }
        observations.add(
            "shield zombie scan: passed"
                + " entity=" + zombie.entityType()
                + " entity_id=" + zombie.entityId()
                + " distance_squared=" + zombie.distanceSquared()
        );

        boolean approached = client.approachEntity(zombie, APPROACH_TIMEOUT);
        observations.add(
            "shield zombie approach: " + (approached ? "passed" : "failed")
                + " entity_id=" + zombie.entityId()
        );
        if (!approached) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        ScenarioShieldBlockResult shieldBlock = client.blockAttackWithSelectedShield(
            "minecraft:shield",
            SHIELD_BLOCK_TIMEOUT
        );
        boolean survived = shieldBlock.healthAfter() > 0.0F;
        boolean blocked = shieldBlock.useStarted()
            && shieldBlock.blockedAttackObserved()
            && survived
            && shieldBlock.healthAfter() >= shieldBlock.healthBefore();
        observations.add(
            "shield zombie block: " + (blocked ? "passed" : "failed")
                + " entity_id=" + zombie.entityId()
                + " shield_use_started=" + shieldBlock.useStarted()
                + " blocked_attack_observed=" + shieldBlock.blockedAttackObserved()
                + " shield_damage_before=" + shieldBlock.shieldDamageBefore()
                + " shield_damage_after=" + shieldBlock.shieldDamageAfter()
                + " health_before=" + shieldBlock.healthBefore()
                + " health_after=" + shieldBlock.healthAfter()
                + " survived=" + survived
        );
        return new ClientScenarioReport(blocked ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runEarnedIronChestplateEquip(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        IronChestplateProgressionResult chestplate = craftEarnedIronChestplateProgressionResult(
            id,
            observations,
            client
        );
        if (!"passed".equals(chestplate.report().result())) {
            return chestplate.report();
        }

        return equipEarnedIronChestplate(id, observations, client);
    }

    private ClientScenarioReport equipEarnedIronChestplate(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        ScenarioHeldItem heldChestplate = client.selectHotbarItem(
            "minecraft:iron_chestplate",
            1,
            HOTBAR_TIMEOUT
        );
        if (!heldChestplate.matches("minecraft:iron_chestplate", 1)) {
            observations.add("blocked: earned iron chestplate exists but is not selectable for armor equip");
            return new ClientScenarioReport("blocked", id, observations);
        }

        boolean equipped = client.quickEquipSelectedArmor(
            "minecraft:iron_chestplate",
            "chest",
            INVENTORY_TIMEOUT
        );
        observations.add(
            "iron chestplate equip: " + (equipped ? "passed" : "failed")
                + " armor_slot=chest"
                + " item=minecraft:iron_chestplate"
                + " quick_move_equipped=" + equipped
        );
        return new ClientScenarioReport(equipped ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runEarnedIronChestplateZombieMitigation(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        ClientScenarioReport chestplate = runEarnedIronChestplateEquip(id, observations, client);
        if (!"passed".equals(chestplate.result())) {
            return chestplate;
        }

        return measureIronChestplateZombieMitigation(
            id,
            observations,
            client,
            "iron chestplate zombie mitigation"
        );
    }

    private ClientScenarioReport measureIronChestplateZombieMitigation(
        String id,
        List<String> observations,
        ScenarioClient client,
        String observationPrefix
    ) throws Exception {
        String entityObservationPrefix = "iron chestplate zombie mitigation".equals(observationPrefix)
            ? "iron chestplate zombie"
            : observationPrefix;
        boolean nightReached = client.waitForDayTimeAtOrAfter(NIGHT_START_DAY_TIME, NIGHT_WAIT_TIMEOUT);
        observations.add(
            "natural night wait for " + observationPrefix + ": " + (nightReached ? "passed" : "failed")
                + " night_start_day_time=" + NIGHT_START_DAY_TIME
        );
        if (!nightReached) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioEntityObservation zombie = client.findVisibleEntity(
            ZOMBIE_ENTITY_IDS,
            ScenarioReach.OUTSIDE_SURVIVAL_REACH,
            ENTITY_SCAN_TIMEOUT
        );
        if (zombie == null) {
            zombie = client.findVisibleEntity(
                ZOMBIE_ENTITY_IDS,
                ScenarioReach.WITHIN_SURVIVAL_REACH,
                ENTITY_SCAN_TIMEOUT
            );
        }
        if (zombie == null) {
            observations.add("blocked: no loaded natural zombie visible after nightfall");
            return new ClientScenarioReport("blocked", id, observations);
        }
        observations.add(
            entityObservationPrefix + " scan: passed"
                + " entity=" + zombie.entityType()
                + " entity_id=" + zombie.entityId()
                + " distance_squared=" + zombie.distanceSquared()
        );

        boolean approached = client.approachEntity(zombie, APPROACH_TIMEOUT);
        observations.add(
            entityObservationPrefix + " approach: " + (approached ? "passed" : "failed")
                + " entity_id=" + zombie.entityId()
        );
        if (!approached) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        float healthBefore = client.playerHealth();
        float healthAfter = client.waitForPlayerHealthBelow(healthBefore, HOSTILE_HIT_TIMEOUT);
        float damageTaken = healthBefore - healthAfter;
        boolean observedHit = damageTaken > 0.0F;
        boolean survived = healthAfter > 0.0F;
        boolean mitigated = damageTaken <= IRON_CHESTPLATE_MAX_ZOMBIE_HIT_DAMAGE;
        boolean passed = observedHit && survived && mitigated;
        observations.add(
            observationPrefix + ": " + (passed ? "passed" : "failed")
                + " entity_id=" + zombie.entityId()
                + " health_before=" + healthBefore
                + " health_after=" + healthAfter
                + " damage_taken=" + damageTaken
                + " max_expected_damage=" + IRON_CHESTPLATE_MAX_ZOMBIE_HIT_DAMAGE
                + " observed_hit=" + observedHit
                + " survived=" + survived
                + " mitigated=" + mitigated
        );
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport mineRawIronWithStonePickaxe(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        ScenarioHeldItem stonePickaxe = client.selectHotbarItem("minecraft:stone_pickaxe", 1, HOTBAR_TIMEOUT);
        if (!stonePickaxe.matches("minecraft:stone_pickaxe", 1)) {
            observations.add("blocked: crafted stone pickaxe is not selectable for iron mining");
            return new ClientScenarioReport("blocked", id, observations);
        }

        int rawIronTarget = client.inventoryCount("minecraft:raw_iron") + 1;
        int miningAttempts = 0;
        int maxMiningAttempts = 4;
        while (
            client.inventoryCount("minecraft:raw_iron") < rawIronTarget
                && miningAttempts < maxMiningAttempts
        ) {
            miningAttempts += 1;
            ScenarioBlockTarget ore = client.findBreakableBlock(
                IRON_ORE_BLOCK_IDS,
                ScenarioReach.WITHIN_SURVIVAL_REACH
            );
            if (ore == null) {
                ScenarioBlockTarget farOre = client.findBreakableBlock(
                    IRON_ORE_BLOCK_IDS,
                    ScenarioReach.OUTSIDE_SURVIVAL_REACH
                );
                if (farOre == null) {
                    observations.add("blocked: no loaded natural iron ore found near the real client");
                    return new ClientScenarioReport("blocked", id, observations);
                }
                boolean approached = client.approachBlock(farOre, APPROACH_TIMEOUT);
                observations.add(
                    "natural iron ore approach: " + (approached ? "passed" : "failed")
                        + " target=" + coordinates(farOre)
                );
                if (!approached) {
                    return new ClientScenarioReport("blocked", id, observations);
                }
                ore = client.findBreakableBlock(IRON_ORE_BLOCK_IDS, ScenarioReach.WITHIN_SURVIVAL_REACH);
                if (ore == null) {
                    observations.add("blocked: natural iron ore remained outside survival reach after approach");
                    return new ClientScenarioReport("blocked", id, observations);
                }
            }

            boolean closeApproached = client.approachBlock(ore, APPROACH_TIMEOUT);
            observations.add(
                "natural iron ore close approach: " + (closeApproached ? "passed" : "failed")
                    + " target=" + coordinates(ore)
            );
            if (!closeApproached) {
                return new ClientScenarioReport("blocked", id, observations);
            }

            int rawIronBefore = client.inventoryCount("minecraft:raw_iron");
            ScenarioBreakResult broke = client.breakBlockUntilDropVisible(ore, "minecraft:raw_iron", BREAK_TIMEOUT);
            ScenarioBreakResult pickup = client.collectVisibleItemDrop(ore, "minecraft:raw_iron", 1, PICKUP_TIMEOUT);
            int rawIronAfter = client.inventoryCount("minecraft:raw_iron");
            boolean inventoryAdvanced = rawIronAfter >= rawIronBefore + 1;
            boolean naturalPickup = broke.started()
                && broke.becameAir()
                && broke.sawDrop()
                && inventoryAdvanced;
            String pickupDetail = pickup.pickupDetail().isBlank()
                ? ""
                : " pickup_detail=" + pickup.pickupDetail();
            observations.add(
                "natural iron ore break/drop/pickup: " + (naturalPickup ? "passed" : "failed")
                    + " target=" + coordinates(ore)
                    + " block=" + ore.blockId()
                    + " break_started=" + broke.started()
                    + " became_air=" + broke.becameAir()
                    + " saw_drop=" + broke.sawDrop()
                    + " pickup_restored=" + pickup.pickupRestored()
                    + " raw_iron_before=" + rawIronBefore
                    + " raw_iron_after=" + rawIronAfter
                    + pickupDetail
            );
            if (!naturalPickup) {
                if (broke.started() && broke.becameAir() && broke.sawDrop() && !inventoryAdvanced) {
                    continue;
                }
                return new ClientScenarioReport("failed", id, observations);
            }
        }

        int rawIronCount = client.inventoryCount("minecraft:raw_iron");
        if (rawIronCount < rawIronTarget) {
            observations.add(
                "natural iron ore inventory: failed raw_iron_count=" + rawIronCount
                    + " expected_at_least=" + rawIronTarget
                    + " mining_attempts=" + miningAttempts
            );
            return new ClientScenarioReport("failed", id, observations);
        }

        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport smeltRawIronInPlacedFurnace(
        String id,
        List<String> observations,
        ScenarioClient client,
        FurnacePlacementOpenResult furnace,
        PlanksRecipe planks
    ) throws Exception {
        return smeltRawIronInPlacedFurnace(id, observations, client, furnace, planks, true, 1);
    }

    private ClientScenarioReport smeltRawIronInPlacedFurnace(
        String id,
        List<String> observations,
        ScenarioClient client,
        FurnacePlacementOpenResult furnace,
        PlanksRecipe planks,
        int smeltCount
    ) throws Exception {
        return smeltRawIronInPlacedFurnace(id, observations, client, furnace, planks, true, smeltCount);
    }

    private ClientScenarioReport smeltRawIronInPlacedFurnace(
        String id,
        List<String> observations,
        ScenarioClient client,
        FurnacePlacementOpenResult furnace,
        PlanksRecipe planks,
        boolean addFuel,
        int smeltCount
    ) throws Exception {
        boolean furnaceApproached = client.approachBlock(furnace.furnaceTarget(), APPROACH_TIMEOUT);
        observations.add(
            "furnace approach for iron ingot: " + (furnaceApproached ? "passed" : "failed")
                + " target=" + coordinates(furnace.furnaceTarget())
        );
        if (!furnaceApproached) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        ScenarioHeldItem stonePickaxe = client.selectHotbarItem("minecraft:stone_pickaxe", 1, HOTBAR_TIMEOUT);
        ScenarioBlockTarget furnaceUseTarget = reachableUseTarget(client, furnace.furnaceTarget());
        ScenarioUseResult openUse = client.useItemOn(furnaceUseTarget, stonePickaxe);
        boolean opened = client.waitForScreenClassName(FURNACE_SCREEN, INVENTORY_TIMEOUT);
        observations.add(
            "furnace reopen for iron ingot: " + (opened ? "passed" : "failed")
                + " open_use_result=" + openUse.result()
                + " screen=" + FURNACE_SCREEN
                + " screen_matched=" + opened
        );
        if (!opened) {
            return new ClientScenarioReport("failed", id, observations);
        }

        int ironIngotCount = client.inventoryCount("minecraft:iron_ingot");
        boolean inputMoved = client.moveSelectedItemToContainerSlot(
            0,
            "minecraft:raw_iron",
            smeltCount,
            INVENTORY_TIMEOUT
        );
        observations.add("furnace raw iron input transfer: " + (inputMoved ? "passed" : "failed"));
        if (!inputMoved) {
            return new ClientScenarioReport("failed", id, observations);
        }

        boolean fuelMoved = true;
        if (addFuel) {
            int fuelCount = smeltCount > 1 ? 2 : 1;
            fuelMoved = client.moveSelectedItemToContainerSlot(
                1,
                planks.planksItemId(),
                fuelCount,
                INVENTORY_TIMEOUT
            );
            observations.add(
                "furnace iron fuel transfer: " + (fuelMoved ? "passed" : "failed")
                    + " slot=1 item=" + planks.planksItemId()
            );
            if (!fuelMoved) {
                return new ClientScenarioReport("failed", id, observations);
            }
        } else {
            observations.add(
                "furnace iron fuel transfer: skipped existing_burn=true"
                    + " slot=1 item=" + planks.planksItemId()
            );
        }

        boolean outputReady = client.waitForContainerSlot(
            2,
            "minecraft:iron_ingot",
            smeltCount,
            FURNACE_COOK_TIMEOUT
        );
        observations.add(
            "furnace iron ingot output: " + (outputReady ? "passed" : "failed")
                + " slot=2 item=minecraft:iron_ingot"
        );
        if (!outputReady) {
            return new ClientScenarioReport("failed", id, observations);
        }

        boolean fuelRemainderCleared = !addFuel || client.moveContainerSlotToInventory(
            1,
            planks.planksItemId(),
            1,
            INVENTORY_TIMEOUT
        ) || client.waitForContainerSlotEmpty(1, INVENTORY_TIMEOUT);
        observations.add(
            "furnace iron fuel remainder clear: " + (fuelRemainderCleared ? "passed" : "failed")
                + " slot=1 item=" + planks.planksItemId()
        );
        if (!fuelRemainderCleared) {
            return new ClientScenarioReport("failed", id, observations);
        }

        boolean inputRemainderCleared = client.waitForContainerSlotEmpty(0, INVENTORY_TIMEOUT)
            || client.moveContainerSlotToInventory(0, "minecraft:raw_iron", 1, INVENTORY_TIMEOUT);
        observations.add(
            "furnace raw iron input remainder clear: " + (inputRemainderCleared ? "passed" : "failed")
                + " slot=0 item=minecraft:raw_iron"
        );
        if (!inputRemainderCleared) {
            return new ClientScenarioReport("failed", id, observations);
        }

        int experienceBefore = smeltCount > 1 ? client.totalExperience() : -1;
        int expectedIronIngotCount = ironIngotCount + smeltCount;
        boolean outputTaken = client.moveContainerSlotToInventory(
            2,
            "minecraft:iron_ingot",
            smeltCount,
            INVENTORY_TIMEOUT
        );
        boolean ingotInInventory = client.waitForInventoryCount(
            "minecraft:iron_ingot",
            expectedIronIngotCount,
            INVENTORY_TIMEOUT
        );
        int experienceAfter = smeltCount > 1
            ? client.waitForTotalExperienceAbove(experienceBefore, INVENTORY_TIMEOUT)
            : experienceBefore;
        boolean experienceAdvanced = smeltCount <= 1 || experienceAfter > experienceBefore;
        boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        observations.add(
            "furnace iron ingot inventory: "
                + (outputTaken && ingotInInventory && closed ? "passed" : "failed")
                + " output_taken=" + outputTaken
                + " iron_ingot_expected_count=" + expectedIronIngotCount
                + " iron_ingot_inventory_matched=" + ingotInInventory
                + " closed=" + closed
        );
        if (smeltCount > 1) {
            observations.add(
                "furnace iron experience: " + (experienceAdvanced ? "passed" : "failed")
                    + " total_experience_before=" + experienceBefore
                    + " total_experience_after=" + experienceAfter
            );
        }
        if (!outputTaken || !ingotInInventory || !experienceAdvanced || !closed) {
            return new ClientScenarioReport("failed", id, observations);
        }
        return new ClientScenarioReport("passed", id, observations);
    }

    private CampfireDeathRespawnResult performEarnedCampfireDeathRespawn(
        String id,
        List<String> observations,
        ScenarioClient client,
        boolean observeWoodenPickaxeDrop
    ) throws Exception {
        CampfireCookingResult cooked = prepareEarnedCampfireCooking(id, observations, client, false);
        if (!"passed".equals(cooked.report().result())) {
            return new CampfireDeathRespawnResult(cooked.report(), cooked.campfireTarget(), null);
        }

        if (cooked.campfireTarget() == null) {
            observations.add("blocked: placed campfire target was not retained for hazard death probe");
            return new CampfireDeathRespawnResult(new ClientScenarioReport("blocked", id, observations), null, null);
        }
        ScenarioBlockTarget campfireTarget = new ScenarioBlockTarget(
            cooked.campfireTarget().x(),
            cooked.campfireTarget().y(),
            cooked.campfireTarget().z(),
            "up",
            "campfire-target",
            cooked.campfireTarget().blockId()
        );
        List<ScenarioItemDropIdentity> preexistingPickaxeIdentities = observeWoodenPickaxeDrop
            ? client.visibleItemDropIdentities("minecraft:wooden_pickaxe")
            : List.of();

        boolean deathScreen = client.standOnBlockUntilDeath(campfireTarget, CAMPFIRE_DEATH_TIMEOUT);
        observations.add(
            "campfire hazard death: " + (deathScreen ? "passed" : "failed")
                + " target=" + coordinates(campfireTarget)
                + " timeout_seconds=" + CAMPFIRE_DEATH_TIMEOUT.toSeconds()
        );
        if (!deathScreen) {
            return new CampfireDeathRespawnResult(
                new ClientScenarioReport("failed", id, observations),
                campfireTarget,
                null
            );
        }

        ScenarioItemDropIdentity woodenPickaxeDropIdentity = observeWoodenPickaxeDrop
            ? client.waitForNewVisibleItemDropIdentity(
                "minecraft:wooden_pickaxe",
                preexistingPickaxeIdentities,
                PICKUP_TIMEOUT
            )
            : null;
        boolean attributableDrop = !observeWoodenPickaxeDrop
            || (woodenPickaxeDropIdentity != null
                && !preexistingPickaxeIdentities.contains(woodenPickaxeDropIdentity));
        if (observeWoodenPickaxeDrop) {
            observations.add(
                "campfire wooden pickaxe death drop: " + (attributableDrop ? "passed" : "failed")
                    + " identity=" + (woodenPickaxeDropIdentity == null ? "missing" : woodenPickaxeDropIdentity)
                    + " preexisting_identities=" + preexistingPickaxeIdentities
            );
        }

        boolean respawned = client.performRespawn(RESPAWN_TIMEOUT);
        observations.add(
            "campfire respawn: " + (respawned ? "passed" : "failed")
                + " timeout_seconds=" + RESPAWN_TIMEOUT.toSeconds()
        );
        return new CampfireDeathRespawnResult(
            new ClientScenarioReport(respawned && attributableDrop ? "passed" : "failed", id, observations),
            campfireTarget,
            woodenPickaxeDropIdentity
        );
    }

    private ChestStorageResult storeEarnedItemInChest(
        String id,
        List<String> observations,
        ScenarioClient client,
        boolean requireScreenClosed
    ) throws Exception {
        LogToPlanksResult planks = runLogToPlanks(id, observations, client, 3, true, false);
        if (!"passed".equals(planks.report().result())) {
            return new ChestStorageResult(planks.report(), null, null, 0);
        }
        CraftingTableOpenResult table = craftPlaceAndOpenTable(
            id,
            observations,
            client,
            planks.planks().planksItemId(),
            false
        );
        if (!"passed".equals(table.report().result())) {
            return new ChestStorageResult(table.report(), null, null, 0);
        }

        int planksCount = client.inventoryCount(planks.planks().planksItemId());
        int chestCount = client.inventoryCount("minecraft:chest");
        if (planksCount < 8) {
            observations.add("chest recipe: failed fewer than eight earned planks available");
            return new ChestStorageResult(new ClientScenarioReport("failed", id, observations), null, null, 0);
        }
        int expectedPlanksAfterChest = planksCount - 8;
        int expectedChestCount = chestCount + 1;
        int containerId = client.activeContainerId();
        client.placeRecipe(containerId, CHEST_RECIPE_DISPLAY_ID, false);
        boolean planksConsumed = client.waitForInventoryCount(
            planks.planks().planksItemId(),
            expectedPlanksAfterChest,
            INVENTORY_TIMEOUT
        );
        boolean chestCreated = client.waitForInventoryCount(
            "minecraft:chest",
            expectedChestCount,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "chest recipe: " + (planksConsumed && chestCreated ? "passed" : "failed")
                + " container_id=" + containerId
                + " recipe_display_id=" + CHEST_RECIPE_DISPLAY_ID
                + " planks_item=" + planks.planks().planksItemId()
                + " planks_expected_count=" + expectedPlanksAfterChest
                + " planks_count_matched=" + planksConsumed
                + " chest_expected_count=" + expectedChestCount
                + " chest_count_matched=" + chestCreated
        );
        if (!planksConsumed || !chestCreated) {
            return new ChestStorageResult(new ClientScenarioReport("failed", id, observations), null, null, 0);
        }
        boolean craftingClosed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        observations.add("crafting table screen close after chest: " + (craftingClosed ? "passed" : "failed"));
        if (!craftingClosed) {
            return new ChestStorageResult(new ClientScenarioReport("failed", id, observations), null, null, 0);
        }

        PassiveFoodDropResult drop = collectPassiveFoodDrop(id, observations, client);
        if (!"passed".equals(drop.report().result())) {
            return new ChestStorageResult(drop.report(), null, null, 0);
        }

        ScenarioHeldItem chest = client.selectHotbarItem("minecraft:chest", 1, HOTBAR_TIMEOUT);
        if (!chest.matches("minecraft:chest", 1)) {
            observations.add("blocked: earned chest exists but is not selectable from hotbar");
            return new ChestStorageResult(new ClientScenarioReport("blocked", id, observations), null, null, 0);
        }
        ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (pair == null) {
            observations.add("blocked: no loaded dry target found for chest placement");
            return new ChestStorageResult(new ClientScenarioReport("blocked", id, observations), null, null, 0);
        }
        ScenarioUseResult placeUse = client.useItemOn(pair.clicked(), chest);
        boolean placed = client.waitForBlock(pair.target(), "minecraft:chest", BLOCK_TIMEOUT);
        ScenarioBlockTarget chestTarget = new ScenarioBlockTarget(
            pair.target().x(),
            pair.target().y(),
            pair.target().z(),
            "up",
            "chest-target",
            "minecraft:chest"
        );

        ScenarioHeldItem storedItem = client.selectHotbarItem(drop.dropItemId(), 1, HOTBAR_TIMEOUT);
        if (!storedItem.matches(drop.dropItemId(), 1)) {
            observations.add("blocked: earned chest storage item exists but is not selectable from hotbar");
            return new ChestStorageResult(new ClientScenarioReport("blocked", id, observations), chestTarget, null, 0);
        }

        ScenarioUseResult openUse = client.useItemOn(chestTarget, storedItem);
        boolean opened = client.waitForScreenClassName(CONTAINER_SCREEN, INVENTORY_TIMEOUT);
        observations.add(
            "chest placement/open: " + (placed && opened ? "passed" : "failed")
                + " place_use_result=" + placeUse.result()
                + " placed=" + placed
                + " open_use_result=" + openUse.result()
                + " screen=" + CONTAINER_SCREEN
                + " screen_matched=" + opened
                + " target=" + coordinates(chestTarget)
        );
        if (!placed || !opened) {
            return new ChestStorageResult(
                new ClientScenarioReport("failed", id, observations),
                chestTarget,
                drop.dropItemId(),
                1
            );
        }

        boolean deposited = client.moveSelectedItemToContainerSlot(
            0,
            drop.dropItemId(),
            1,
            INVENTORY_TIMEOUT
        );
        boolean slotMatched = deposited
            && client.waitForContainerSlot(0, drop.dropItemId(), 1, INVENTORY_TIMEOUT);
        boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        boolean passed = deposited && slotMatched && (closed || !requireScreenClosed);
        observations.add(
            "earned chest storage: " + (passed ? "passed" : "failed")
                + " slot=0"
                + " item=" + drop.dropItemId()
                + " moved=" + deposited
                + " slot_matched=" + slotMatched
                + " closed=" + closed
                + " close_required=" + requireScreenClosed
        );
        return new ChestStorageResult(
            new ClientScenarioReport(passed ? "passed" : "failed", id, observations),
            chestTarget,
            drop.dropItemId(),
            1
        );
    }

    private ClientScenarioReport runChestStorageSaveRestartBefore(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        ChestStorageResult stored = storeEarnedItemInChest(id, observations, client, false);
        if (!"passed".equals(stored.report().result())) {
            return stored.report();
        }
        if (stored.chestTarget() == null || stored.itemId() == null || stored.count() <= 0) {
            observations.add("chest storage marker: failed missing stored chest target or item");
            return new ClientScenarioReport("failed", id, observations);
        }

        writeChestStorageMarker(chestStorageMarkerPath(screenshotsDir), stored);
        observations.add(
            "chest storage marker: passed target=" + coordinates(stored.chestTarget())
                + " item=" + stored.itemId()
                + " count=" + stored.count()
        );
        observations.add("runner-managed restart: pending clean server restart and chest storage rejoin check");
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runChestStorageSaveRestartAfter(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = chestStorageMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing chest storage marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        ChestStorageMarker marker = readChestStorageMarker(markerPath);
        boolean chestPersisted = client.waitForBlock(marker.chestTarget(), "minecraft:chest", BLOCK_TIMEOUT);
        observations.add(
            "chest block persistence: " + (chestPersisted ? "passed" : "failed")
                + " target=" + coordinates(marker.chestTarget())
        );
        ScenarioUseResult openUse = new ScenarioUseResult("skipped");
        boolean opened = false;
        boolean slotMatched = false;
        boolean closed = false;
        if (chestPersisted) {
            openUse = client.useItemOn(marker.chestTarget(), client.selectedItem());
            opened = client.waitForScreenClassName(CONTAINER_SCREEN, INVENTORY_TIMEOUT);
            slotMatched = opened
                && client.waitForContainerSlot(0, marker.itemId(), marker.count(), INVENTORY_TIMEOUT);
            closed = opened && client.closeCurrentScreen(INVENTORY_TIMEOUT);
        }
        observations.add(
            "chest storage reopen: " + (opened ? "passed" : "failed")
                + " open_use_result=" + openUse.result()
                + " screen=" + CONTAINER_SCREEN
                + " screen_matched=" + opened
        );
        boolean passed = chestPersisted && opened && slotMatched && closed;
        observations.add(
            "chest storage persistence: " + (passed ? "passed" : "failed")
                + " slot=0"
                + " item=" + marker.itemId()
                + " count=" + marker.count()
                + " slot_matched=" + slotMatched
                + " closed=" + closed
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runGeneratedRuinCacheBefore(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        boolean reachedCenter = client.approachPosition(
            GENERATED_RUIN_CENTER_X,
            GENERATED_RUIN_CENTER_Z,
            APPROACH_TIMEOUT
        );
        observations.add(
            "generated ruin center approach: " + (reachedCenter ? "passed" : "failed")
                + " x=" + GENERATED_RUIN_CENTER_X
                + " z=" + GENERATED_RUIN_CENTER_Z
        );
        if (!reachedCenter) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioBlockTarget chest = client.findLoadedBlockInColumn(
            GENERATED_RUIN_CENTER_X,
            GENERATED_RUIN_CENTER_Z,
            List.of("minecraft:chest")
        );
        boolean exactChest = chest != null
            && chest.x() == GENERATED_RUIN_CENTER_X
            && chest.z() == GENERATED_RUIN_CENTER_Z
            && "minecraft:chest".equals(chest.blockId());
        observations.add(
            "exact generated chest Y: " + (exactChest ? "passed" : "failed")
                + (chest == null ? "" : " target=" + coordinates(chest))
        );
        if (!exactChest) {
            return new ClientScenarioReport("failed", id, observations);
        }

        boolean approachedChest = client.approachBlock(chest, APPROACH_TIMEOUT);
        observations.add(
            "generated ruin chest approach: " + (approachedChest ? "passed" : "failed")
                + " target=" + coordinates(chest)
        );
        if (!approachedChest) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioUseResult openUse = client.useItemOn(chest, client.selectedItem());
        boolean opened = client.waitForScreenClassName(CONTAINER_SCREEN, INVENTORY_TIMEOUT);
        int containerId = opened ? client.activeContainerId() : -1;
        boolean openedFromClientState = opened && containerId > 0;
        observations.add(
            "generated ruin chest open client-state: " + (openedFromClientState ? "passed" : "failed")
                + " use_result=" + openUse.result()
                + " container_id=" + containerId
        );
        if (!openedFromClientState) {
            return new ClientScenarioReport("failed", id, observations);
        }

        List<GeneratedRuinLoot> movedLoot = new ArrayList<>();
        boolean lootMoved = true;
        for (GeneratedRuinLoot expected : GENERATED_RUIN_LOOT) {
            int inventoryBefore = client.inventoryCount(expected.itemId());
            int slot = client.findContainerSlot(expected.itemId(), expected.count());
            boolean emptyInventoryBefore = inventoryBefore == 0;
            boolean moved = emptyInventoryBefore
                && slot >= 0
                && client.quickMoveContainerSlot(slot, INVENTORY_TIMEOUT);
            boolean chestSlotEmpty = moved && client.waitForContainerSlotEmpty(slot, INVENTORY_TIMEOUT);
            boolean inventoryMatched = chestSlotEmpty
                && client.waitForInventoryCount(expected.itemId(), expected.count(), INVENTORY_TIMEOUT);
            boolean itemPassed = emptyInventoryBefore && slot >= 0 && moved && chestSlotEmpty && inventoryMatched;
            observations.add(
                "generated ruin loot quick-move: " + (itemPassed ? "passed" : "failed")
                    + " item=" + expected.itemId()
                    + " count=" + expected.count()
                    + " slot=" + slot
                    + " inventory_before=" + inventoryBefore
                    + " authoritative_move=" + moved
                    + " chest_slot_empty=" + chestSlotEmpty
                    + " inventory_matched=" + inventoryMatched
            );
            lootMoved &= itemPassed;
            movedLoot.add(new GeneratedRuinLoot(expected.itemId(), expected.count(), slot));
        }
        boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        observations.add("generated ruin chest close client-state: " + (closed ? "passed" : "failed"));
        if (!lootMoved || !closed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        writeGeneratedRuinCacheMarker(
            generatedRuinCacheMarkerPath(screenshotsDir),
            new GeneratedRuinCacheMarker(chest, movedLoot)
        );
        observations.add("runner-managed restart: pending clean server restart and generated ruin cache rejoin check");
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runGeneratedRuinCacheAfter(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = generatedRuinCacheMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing generated ruin cache marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        GeneratedRuinCacheMarker marker = readGeneratedRuinCacheMarker(markerPath);
        boolean reachedCenter = client.approachPosition(
            GENERATED_RUIN_CENTER_X,
            GENERATED_RUIN_CENTER_Z,
            APPROACH_TIMEOUT
        );
        boolean chestPersisted = reachedCenter
            && client.waitForBlock(marker.chestTarget(), "minecraft:chest", BLOCK_TIMEOUT);
        observations.add(
            "generated ruin chest persistence client-state: " + (chestPersisted ? "passed" : "failed")
                + " target=" + coordinates(marker.chestTarget())
        );
        if (!chestPersisted || !client.approachBlock(marker.chestTarget(), APPROACH_TIMEOUT)) {
            observations.add("generated ruin chest re-approach: failed");
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioUseResult openUse = client.useItemOn(marker.chestTarget(), client.selectedItem());
        boolean opened = client.waitForScreenClassName(CONTAINER_SCREEN, INVENTORY_TIMEOUT);
        int containerId = opened ? client.activeContainerId() : -1;
        boolean openedFromClientState = opened && containerId > 0;
        observations.add(
            "generated ruin chest reopen client-state: " + (openedFromClientState ? "passed" : "failed")
                + " use_result=" + openUse.result()
                + " container_id=" + containerId
        );
        if (!openedFromClientState) {
            return new ClientScenarioReport("failed", id, observations);
        }

        boolean inventoryMatched = true;
        for (GeneratedRuinLoot expected : marker.loot()) {
            boolean itemInventoryMatched = client.waitForInventoryCount(
                expected.itemId(),
                expected.count(),
                INVENTORY_TIMEOUT
            );
            inventoryMatched &= itemInventoryMatched;
            observations.add(
                "generated ruin persistence item: "
                    + (itemInventoryMatched ? "passed" : "failed")
                    + " item=" + expected.itemId()
                    + " count=" + expected.count()
                    + " slot=" + expected.slot()
                    + " inventory_matched=" + itemInventoryMatched
            );
        }
        boolean chestSlotsEmpty = true;
        for (int slot = 0; slot < GENERATED_RUIN_CHEST_SLOT_COUNT; slot++) {
            boolean slotEmpty = client.waitForContainerSlotEmpty(slot, INVENTORY_TIMEOUT);
            chestSlotsEmpty &= slotEmpty;
            observations.add(
                "generated ruin chest slot empty: " + (slotEmpty ? "passed" : "failed")
                    + " slot=" + slot
            );
        }
        boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        boolean passed = inventoryMatched && chestSlotsEmpty && closed;
        observations.add(
            "generated ruin cache persistence: " + (passed ? "passed" : "failed")
                + " inventory_counts_matched=" + inventoryMatched
                + " chest_slots_empty=" + chestSlotsEmpty
                + " closed=" + closed
        );
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runStonecutterConservation(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        ScenarioHeldItem station = client.giveAndSelect(
            "minecraft:stonecutter",
            1,
            0,
            HOTBAR_TIMEOUT
        );
        ScenarioBlockPair pair = client.findUnobstructedPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (!station.matches("minecraft:stonecutter", 1) || pair == null) {
            observations.add("stonecutter setup: failed station=" + station.itemId() + " x" + station.count());
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioUseResult placeUse = client.useItemOn(pair.clicked(), station);
        boolean placed = client.waitForBlock(pair.target(), "minecraft:stonecutter", BLOCK_TIMEOUT);
        ScenarioBlockTarget stonecutter = new ScenarioBlockTarget(
            pair.target().x(),
            pair.target().y(),
            pair.target().z(),
            "up",
            "stonecutter-target",
            "minecraft:stonecutter"
        );
        ScenarioHeldItem firstInput = client.giveAndSelect(
            STONECUTTER_INPUT_ITEM_ID,
            1,
            1,
            HOTBAR_TIMEOUT
        );
        observations.add(
            "stonecutter setup: " + (placed && firstInput.matches(STONECUTTER_INPUT_ITEM_ID, 1) ? "passed" : "failed")
                + " place_use_result=" + placeUse.result()
                + " input=" + firstInput.itemId() + " x" + firstInput.count()
        );
        if (!placed || !firstInput.matches(STONECUTTER_INPUT_ITEM_ID, 1)) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioUseResult openUse = client.useItemOn(stonecutter, firstInput);
        boolean opened = client.waitForScreenClassName(STONECUTTER_SCREEN, INVENTORY_TIMEOUT);
        int firstContainerId = opened ? client.activeContainerId() : -1;
        boolean openedFromClientState = opened && firstContainerId > 0;
        observations.add(
            "stonecutter menu open: " + (openedFromClientState ? "passed" : "failed")
                + " use_result=" + openUse.result()
                + " container_id=" + firstContainerId
        );
        if (!openedFromClientState) {
            return new ClientScenarioReport("failed", id, observations);
        }

        boolean initialInputMoved = client.moveSelectedItemToContainerSlot(
            STONECUTTER_INPUT_SLOT,
            STONECUTTER_INPUT_ITEM_ID,
            1,
            INVENTORY_TIMEOUT
        );
        boolean initialInputVisible = initialInputMoved && client.waitForContainerSlot(
            STONECUTTER_INPUT_SLOT,
            STONECUTTER_INPUT_ITEM_ID,
            1,
            INVENTORY_TIMEOUT
        );
        int initialOfferId = initialInputVisible ? selectStonecutterSlabOffer(client) : -1;
        boolean initialOutputVisible = initialOfferId >= 0;
        boolean normalPickup = initialOutputVisible && client.moveContainerSlotToInventory(
            STONECUTTER_OUTPUT_SLOT,
            STONECUTTER_OUTPUT_ITEM_ID,
            2,
            INVENTORY_TIMEOUT
        );
        boolean normalConserved = normalPickup
            && client.waitForContainerSlotEmpty(STONECUTTER_INPUT_SLOT, INVENTORY_TIMEOUT)
            && client.waitForContainerSlotEmpty(STONECUTTER_OUTPUT_SLOT, INVENTORY_TIMEOUT)
            && client.waitForInventoryCount(STONECUTTER_INPUT_ITEM_ID, 0, INVENTORY_TIMEOUT)
            && client.waitForInventoryCount(STONECUTTER_OUTPUT_ITEM_ID, 2, INVENTORY_TIMEOUT);
        observations.add(
            "stonecutter normal pickup: " + (normalConserved ? "passed" : "failed")
                + " offer_id=" + initialOfferId
                + " input_consumed=1 output_added=2"
        );
        if (!normalConserved) {
            return new ClientScenarioReport("failed", id, observations);
        }

        boolean closedAfterNormalPickup = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        ScenarioUseResult reopenUse = closedAfterNormalPickup
            ? client.useItemOn(stonecutter, client.selectedItem())
            : new ScenarioUseResult("skipped");
        boolean reopened = closedAfterNormalPickup
            && client.waitForScreenClassName(STONECUTTER_SCREEN, INVENTORY_TIMEOUT);
        int reopenedContainerId = reopened ? client.activeContainerId() : -1;
        boolean reopenedConserved = reopened
            && reopenedContainerId > 0
            && reopenedContainerId != firstContainerId
            && client.waitForContainerSlotEmpty(STONECUTTER_INPUT_SLOT, INVENTORY_TIMEOUT)
            && client.waitForContainerSlotEmpty(STONECUTTER_OUTPUT_SLOT, INVENTORY_TIMEOUT)
            && client.waitForInventoryCount(STONECUTTER_INPUT_ITEM_ID, 0, INVENTORY_TIMEOUT)
            && client.waitForInventoryCount(STONECUTTER_OUTPUT_ITEM_ID, 2, INVENTORY_TIMEOUT);
        observations.add(
            "stonecutter close/reopen conservation: " + (reopenedConserved ? "passed" : "failed")
                + " reopen_use_result=" + reopenUse.result()
                + " first_container_id=" + firstContainerId
                + " reopened_container_id=" + reopenedContainerId
        );
        if (!reopenedConserved) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioHeldItem quickMoveInput = client.giveAndSelect(
            STONECUTTER_INPUT_ITEM_ID,
            2,
            1,
            HOTBAR_TIMEOUT
        );
        boolean quickInputMoved = quickMoveInput.matches(STONECUTTER_INPUT_ITEM_ID, 2)
            && client.moveSelectedItemToContainerSlot(
                STONECUTTER_INPUT_SLOT,
                STONECUTTER_INPUT_ITEM_ID,
                2,
                INVENTORY_TIMEOUT
            );
        int quickOfferId = quickInputMoved ? selectStonecutterSlabOffer(client) : -1;
        boolean quickOutputVisible = quickOfferId >= 0;
        boolean quickMoved = quickOutputVisible
            && client.quickMoveContainerSlot(STONECUTTER_OUTPUT_SLOT, INVENTORY_TIMEOUT);
        boolean exactConservation = quickMoved
            && client.waitForContainerSlotEmpty(STONECUTTER_INPUT_SLOT, INVENTORY_TIMEOUT)
            && client.waitForContainerSlotEmpty(STONECUTTER_OUTPUT_SLOT, INVENTORY_TIMEOUT)
            && client.waitForInventoryCount(STONECUTTER_INPUT_ITEM_ID, 0, INVENTORY_TIMEOUT)
            && client.waitForInventoryCount(STONECUTTER_OUTPUT_ITEM_ID, 6, INVENTORY_TIMEOUT);
        boolean closed = exactConservation && client.closeCurrentScreen(INVENTORY_TIMEOUT);
        boolean passed = exactConservation && closed;
        observations.add(
            "stonecutter conservation: " + (passed ? "passed" : "failed")
                + " offer_id=" + quickOfferId
                + " input_total=3 input_remaining=0 output_expected=6 quick_move=" + quickMoved
                + " closed=" + closed
        );
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private int selectStonecutterSlabOffer(ScenarioClient client) throws Exception {
        for (int offerId = 0; offerId < STONECUTTER_COBBLESTONE_OFFER_COUNT; offerId++) {
            boolean selected = client.clickContainerButton(offerId, INVENTORY_TIMEOUT);
            if (
                selected
                    && client.waitForContainerSlot(
                        STONECUTTER_OUTPUT_SLOT,
                        STONECUTTER_OUTPUT_ITEM_ID,
                        2,
                        Duration.ZERO
                    )
            ) {
                return offerId;
            }
        }
        return -1;
    }

    private ClientScenarioReport runEarnedBedSleep(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        LogToPlanksResult planks = runLogToPlanks(id, observations, client, 2, true, false);
        if (!"passed".equals(planks.report().result())) {
            return planks.report();
        }

        WoolCollectionResult wool = collectSheepWool(id, observations, client, 3);
        if (!"passed".equals(wool.report().result())) {
            return wool.report();
        }

        CraftingTableOpenResult table = craftPlaceAndOpenTable(
            id,
            observations,
            client,
            planks.planks().planksItemId(),
            false
        );
        if (!"passed".equals(table.report().result())) {
            return table.report();
        }

        ClientScenarioReport bed = craftBedInOpenTable(
            id,
            observations,
            client,
            planks.planks().planksItemId(),
            wool.bedRecipe()
        );
        if (!"passed".equals(bed.result())) {
            return bed;
        }

        return placeAndSleepInEarnedBed(id, observations, client, wool.bedRecipe().bedItemId());
    }

    private WoolCollectionResult collectSheepWool(
        String id,
        List<String> observations,
        ScenarioClient client,
        int targetWoolCount
    ) throws Exception {
        ScenarioEntityObservation firstSheep = client.findVisibleEntity(
            SHEEP_WOOL_ENTITY_IDS,
            ScenarioReach.OUTSIDE_SURVIVAL_REACH,
            ENTITY_SCAN_TIMEOUT
        );
        if (firstSheep == null) {
            firstSheep = client.findVisibleEntity(
                SHEEP_WOOL_ENTITY_IDS,
                ScenarioReach.WITHIN_SURVIVAL_REACH,
                ENTITY_SCAN_TIMEOUT
            );
        }
        int woolBefore = client.inventoryCount(EARNED_BED_WOOL_ITEM_ID);
        int requiredWoolCount = woolBefore + targetWoolCount;
        ScenarioEntityObservation sheep = firstSheep != null
            && EARNED_BED_WOOL_ITEM_ID.equals(firstSheep.sheepWoolItemId())
            ? firstSheep
            : null;
        if (firstSheep != null && sheep == null) {
            observations.add(
                "sheep wool scan: skipped"
                    + " entity=" + firstSheep.entityType()
                    + " entity_id=" + firstSheep.entityId()
                    + " wool_item=" + firstSheep.sheepWoolItemId()
                    + " required_wool_item=" + EARNED_BED_WOOL_ITEM_ID
            );
        }
        while (client.inventoryCount(EARNED_BED_WOOL_ITEM_ID) < requiredWoolCount) {
            if (sheep == null) {
                sheep = client.findVisibleSheepWithWool(
                    EARNED_BED_WOOL_ITEM_ID,
                    ScenarioReach.OUTSIDE_SURVIVAL_REACH,
                    ENTITY_SCAN_TIMEOUT
                );
            }
            if (sheep == null) {
                sheep = client.findVisibleSheepWithWool(
                    EARNED_BED_WOOL_ITEM_ID,
                    ScenarioReach.WITHIN_SURVIVAL_REACH,
                    ENTITY_SCAN_TIMEOUT
                );
            }
            if (sheep == null) {
                observations.add("blocked: no loaded natural sheep visible with wool=" + EARNED_BED_WOOL_ITEM_ID);
                return new WoolCollectionResult(new ClientScenarioReport("blocked", id, observations), null);
            }
            observations.add(
                "sheep wool scan: passed"
                    + " entity=" + sheep.entityType()
                    + " entity_id=" + sheep.entityId()
                    + " wool_item=" + EARNED_BED_WOOL_ITEM_ID
                    + " distance_squared=" + sheep.distanceSquared()
            );

            boolean approached = client.approachEntity(sheep, APPROACH_TIMEOUT);
            observations.add(
                "sheep wool approach: " + (approached ? "passed" : "failed")
                    + " entity_id=" + sheep.entityId()
            );
            if (!approached) {
                return new WoolCollectionResult(new ClientScenarioReport("blocked", id, observations), null);
            }

            int woolCountBeforeAttack = client.inventoryCount(EARNED_BED_WOOL_ITEM_ID);
            ScenarioBreakResult attack = client.attackEntityUntilDropCollected(
                sheep,
                EARNED_BED_WOOL_ITEM_ID,
                1,
                ENTITY_ATTACK_TIMEOUT
            );
            int woolCountAfterAttack = client.inventoryCount(EARNED_BED_WOOL_ITEM_ID);
            boolean collected = attack.started()
                && attack.becameAir()
                && attack.pickupRestored()
                && woolCountAfterAttack >= woolCountBeforeAttack + 1;
            observations.add(
                "sheep wool drop: " + (collected ? "passed" : "failed")
                    + " entity_id=" + sheep.entityId()
                    + " attack_started=" + attack.started()
                    + " entity_removed=" + attack.becameAir()
                    + " saw_drop=" + attack.sawDrop()
                    + " pickup_restored=" + attack.pickupRestored()
                    + " wool_count_before=" + woolCountBeforeAttack
                    + " wool_count_after=" + woolCountAfterAttack
                    + " wool_required=" + requiredWoolCount
            );
            if (!collected) {
                return new WoolCollectionResult(new ClientScenarioReport("failed", id, observations), null);
            }
            sheep = null;
        }

        observations.add(
            "earned wool total: passed item=" + EARNED_BED_WOOL_ITEM_ID + " count="
                + client.inventoryCount(EARNED_BED_WOOL_ITEM_ID)
                + " required=" + requiredWoolCount
        );
        BedRecipe bedRecipe = new BedRecipe(
            EARNED_BED_WOOL_ITEM_ID,
            EARNED_BED_ITEM_ID,
            WHITE_BED_RECIPE_DISPLAY_ID
        );
        return new WoolCollectionResult(new ClientScenarioReport("passed", id, observations), bedRecipe);
    }

    private ClientScenarioReport craftBedInOpenTable(
        String id,
        List<String> observations,
        ScenarioClient client,
        String planksItemId,
        BedRecipe bedRecipe
    ) throws Exception {
        int containerId = client.activeContainerId();
        int planksCount = client.inventoryCount(planksItemId);
        int woolCount = client.inventoryCount(bedRecipe.woolItemId());
        int bedCount = client.inventoryCount(bedRecipe.bedItemId());
        if (planksCount < 3 || woolCount < 3) {
            observations.add("bed recipe: failed missing earned planks or wool=" + bedRecipe.woolItemId());
            return new ClientScenarioReport("failed", id, observations);
        }
        int expectedPlanksCount = planksCount - 3;
        int expectedWoolCount = woolCount - 3;
        int expectedBedCount = bedCount + 1;
        client.placeRecipe(containerId, bedRecipe.recipeDisplayId(), false);
        boolean planksConsumed = client.waitForInventoryCount(
            planksItemId,
            expectedPlanksCount,
            INVENTORY_TIMEOUT
        );
        boolean woolConsumed = client.waitForInventoryCount(
            bedRecipe.woolItemId(),
            expectedWoolCount,
            INVENTORY_TIMEOUT
        );
        boolean bedCreated = client.waitForInventoryCount(
            bedRecipe.bedItemId(),
            expectedBedCount,
            INVENTORY_TIMEOUT
        );
        boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        boolean passed = planksConsumed && woolConsumed && bedCreated && closed;
        observations.add(
            "bed recipe: " + (passed ? "passed" : "failed")
                + " container_id=" + containerId
                + " recipe_display_id=" + bedRecipe.recipeDisplayId()
                + " planks_item=" + planksItemId
                + " wool_item=" + bedRecipe.woolItemId()
                + " bed_item=" + bedRecipe.bedItemId()
                + " planks_expected_count=" + expectedPlanksCount
                + " planks_count_matched=" + planksConsumed
                + " wool_expected_count=" + expectedWoolCount
                + " wool_count_matched=" + woolConsumed
                + " bed_expected_count=" + expectedBedCount
                + " bed_count_matched=" + bedCreated
                + " closed=" + closed
        );
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport placeAndSleepInEarnedBed(
        String id,
        List<String> observations,
        ScenarioClient client,
        String bedItemId
    ) throws Exception {
        ScenarioHeldItem bed = client.selectHotbarItem(bedItemId, 1, HOTBAR_TIMEOUT);
        if (!bed.matches(bedItemId, 1)) {
            observations.add("blocked: earned bed exists but is not selectable from hotbar item=" + bedItemId);
            return new ClientScenarioReport("blocked", id, observations);
        }
        ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (pair == null) {
            observations.add("blocked: no loaded dry target found for earned bed placement");
            return new ClientScenarioReport("blocked", id, observations);
        }

        ScenarioUseResult placeUse = client.useItemOn(pair.clicked(), bed);
        boolean placed = client.waitForBlock(pair.target(), bedItemId, BLOCK_TIMEOUT);
        ScenarioBlockTarget bedTarget = new ScenarioBlockTarget(
            pair.target().x(),
            pair.target().y(),
            pair.target().z(),
            "up",
            "bed-target",
            bedItemId
        );
        observations.add(
            "bed placement: " + (placed ? "passed" : "failed")
                + " place_use_result=" + placeUse.result()
                + " target=" + coordinates(bedTarget)
        );
        if (!placed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        boolean nightReached = client.waitForDayTimeAtOrAfter(NIGHT_START_DAY_TIME, NIGHT_WAIT_TIMEOUT);
        observations.add(
            "natural night wait: " + (nightReached ? "passed" : "failed")
                + " night_start_day_time=" + NIGHT_START_DAY_TIME
        );
        if (!nightReached) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioUseResult sleepUse = client.useItemOn(bedTarget, client.selectedItem());
        boolean morningReached = client.waitForDayTimeBelow(NIGHT_START_DAY_TIME, MORNING_WAIT_TIMEOUT);
        observations.add(
            "bed sleep skip: " + (morningReached ? "passed" : "failed")
                + " sleep_use_result=" + sleepUse.result()
                + " morning_day_time_below=" + NIGHT_START_DAY_TIME
        );
        return new ClientScenarioReport(morningReached ? "passed" : "failed", id, observations);
    }

    private PassiveFoodDropResult collectPassiveFoodDrop(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        ScenarioEntityObservation passive = client.findVisibleEntity(
            PASSIVE_FOOD_ENTITY_IDS,
            ScenarioReach.OUTSIDE_SURVIVAL_REACH,
            ENTITY_SCAN_TIMEOUT
        );
        if (passive == null) {
            passive = client.findVisibleEntity(
                PASSIVE_FOOD_ENTITY_IDS,
                ScenarioReach.WITHIN_SURVIVAL_REACH,
                ENTITY_SCAN_TIMEOUT
            );
        }
        if (passive == null) {
            observations.add("blocked: no loaded natural passive food mob visible to the real client");
            return new PassiveFoodDropResult(new ClientScenarioReport("blocked", id, observations), null, 0);
        }

        String dropItemId = PASSIVE_FOOD_DROPS.get(passive.entityType());
        if (dropItemId == null) {
            observations.add("blocked: passive mob has no embedded food drop mapping entity=" + passive.entityType());
            return new PassiveFoodDropResult(new ClientScenarioReport("blocked", id, observations), null, 0);
        }
        observations.add(
            "passive mob scan: passed"
                + " entity=" + passive.entityType()
                + " entity_id=" + passive.entityId()
                + " distance_squared=" + passive.distanceSquared()
                + " expected_drop=" + dropItemId
        );

        boolean approached = client.approachEntity(passive, APPROACH_TIMEOUT);
        observations.add(
            "passive mob approach: " + (approached ? "passed" : "failed")
                + " entity=" + passive.entityType()
                + " entity_id=" + passive.entityId()
        );
        if (!approached) {
            return new PassiveFoodDropResult(new ClientScenarioReport("blocked", id, observations), dropItemId, 0);
        }

        int foodCountBefore = client.inventoryCount(dropItemId);
        ScenarioBreakResult attack = client.attackEntityUntilDropCollected(
            passive,
            dropItemId,
            1,
            ENTITY_ATTACK_TIMEOUT
        );
        int foodCountAfter = client.inventoryCount(dropItemId);
        boolean collected = attack.started()
            && attack.becameAir()
            && attack.pickupRestored()
            && foodCountAfter >= foodCountBefore + 1;
        observations.add(
            "passive food drop: " + (collected ? "passed" : "failed")
                + " entity=" + passive.entityType()
                + " entity_id=" + passive.entityId()
                + " expected_drop=" + dropItemId
                + " attack_started=" + attack.started()
                + " entity_removed=" + attack.becameAir()
                + " saw_drop=" + attack.sawDrop()
                + " pickup_restored=" + attack.pickupRestored()
                + " food_count_before=" + foodCountBefore
                + " food_count_after=" + foodCountAfter
        );
        return new PassiveFoodDropResult(
            new ClientScenarioReport(collected ? "passed" : "failed", id, observations),
            dropItemId,
            foodCountAfter
        );
    }

    private ClientScenarioReport collectNaturalLogItem(
        String id,
        List<String> observations,
        ScenarioClient client,
        PlanksRecipe planks
    ) throws Exception {
        return collectNaturalLogItem(id, observations, client, planks, "furnace input log");
    }

    private ClientScenarioReport collectNaturalLogItem(
        String id,
        List<String> observations,
        ScenarioClient client,
        PlanksRecipe planks,
        String observationLabel
    ) throws Exception {
        ScenarioBlockTarget log = client.findBreakableBlock(
            List.of(planks.logItemId()),
            ScenarioReach.WITHIN_SURVIVAL_REACH
        );
        if (log == null) {
            ScenarioBlockTarget farLog = client.findBreakableBlock(
                List.of(planks.logItemId()),
                ScenarioReach.OUTSIDE_SURVIVAL_REACH
            );
            if (farLog == null) {
                observations.add("blocked: no loaded natural log found for furnace input");
                return new ClientScenarioReport("blocked", id, observations);
            }
            boolean approached = client.approachBlock(farLog, APPROACH_TIMEOUT);
            observations.add(
                observationLabel + " approach: " + (approached ? "passed" : "failed")
                    + " target=" + coordinates(farLog)
            );
            log = client.findBreakableBlock(List.of(planks.logItemId()), ScenarioReach.WITHIN_SURVIVAL_REACH);
            if (!approached) {
                if (log == null) {
                    return new ClientScenarioReport("blocked", id, observations);
                }
                observations.add(
                    observationLabel + " reachable fallback after failed approach: passed"
                        + " target=" + coordinates(log)
                );
            } else if (log == null) {
                observations.add("blocked: furnace input log remained outside survival reach after approach");
                return new ClientScenarioReport("blocked", id, observations);
            }
        }

        boolean closeApproached = client.approachBlock(log, APPROACH_TIMEOUT);
        observations.add(
            observationLabel + " close approach: " + (closeApproached ? "passed" : "failed")
                + " target=" + coordinates(log)
        );
        ScenarioBreakResult broke = client.breakBlockUntilDropVisible(log, planks.logItemId(), BREAK_TIMEOUT);
        ScenarioBreakResult pickup = client.collectVisibleItemDrop(
            log,
            planks.logItemId(),
            1,
            PICKUP_TIMEOUT
        );
        boolean naturalPickup = broke.started()
            && broke.becameAir()
            && broke.sawDrop()
            && pickup.pickupRestored();
        observations.add(
            observationLabel + " break/drop/pickup: " + (naturalPickup ? "passed" : "failed")
                + " target=" + coordinates(log)
                + " break_started=" + broke.started()
                + " became_air=" + broke.becameAir()
                + " saw_drop=" + broke.sawDrop()
                + " pickup_restored=" + pickup.pickupRestored()
        );
        if (!naturalPickup) {
            return new ClientScenarioReport("failed", id, observations);
        }
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport craftWoodenPickaxeInOpenTable(
        String id,
        List<String> observations,
        ScenarioClient client,
        String planksItemId,
        boolean closeScreen
    ) throws Exception {
        int containerId = client.activeContainerId();
        int planksCount = client.inventoryCount(planksItemId);
        int stickCount = client.inventoryCount("minecraft:stick");
        if (planksCount < 2) {
            observations.add("stick recipe: failed fewer than two planks available");
            return new ClientScenarioReport("failed", id, observations);
        }
        int expectedPlanksAfterSticks = planksCount - 2;
        int expectedStickCount = stickCount + 4;
        client.placeRecipe(containerId, STICK_RECIPE_DISPLAY_ID, false);
        boolean stickPlanksConsumed = client.waitForInventoryCount(
            planksItemId,
            expectedPlanksAfterSticks,
            INVENTORY_TIMEOUT
        );
        boolean sticksCreated = client.waitForInventoryCount("minecraft:stick", expectedStickCount, INVENTORY_TIMEOUT);
        observations.add(
            "stick recipe: " + (stickPlanksConsumed && sticksCreated ? "passed" : "failed")
                + " container_id=" + containerId
                + " recipe_display_id=" + STICK_RECIPE_DISPLAY_ID
                + " planks_item=" + planksItemId
                + " planks_expected_count=" + expectedPlanksAfterSticks
                + " planks_count_matched=" + stickPlanksConsumed
                + " stick_expected_count=" + expectedStickCount
                + " stick_count_matched=" + sticksCreated
        );
        if (!stickPlanksConsumed || !sticksCreated) {
            return new ClientScenarioReport("failed", id, observations);
        }

        planksCount = client.inventoryCount(planksItemId);
        stickCount = client.inventoryCount("minecraft:stick");
        int woodenPickaxeCount = client.inventoryCount("minecraft:wooden_pickaxe");
        if (planksCount < 3 || stickCount < 2) {
            observations.add("wooden pickaxe recipe: failed missing planks or sticks");
            return new ClientScenarioReport("failed", id, observations);
        }
        int expectedPlanksAfterPickaxe = planksCount - 3;
        int expectedStickAfterPickaxe = stickCount - 2;
        int expectedWoodenPickaxeCount = woodenPickaxeCount + 1;
        client.placeRecipe(containerId, WOODEN_PICKAXE_RECIPE_DISPLAY_ID, false);
        boolean pickaxePlanksConsumed = client.waitForInventoryCount(
            planksItemId,
            expectedPlanksAfterPickaxe,
            INVENTORY_TIMEOUT
        );
        boolean pickaxeSticksConsumed = client.waitForInventoryCount(
            "minecraft:stick",
            expectedStickAfterPickaxe,
            INVENTORY_TIMEOUT
        );
        boolean pickaxeCreated = client.waitForInventoryCount(
            "minecraft:wooden_pickaxe",
            expectedWoodenPickaxeCount,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "wooden pickaxe recipe: "
                + (pickaxePlanksConsumed && pickaxeSticksConsumed && pickaxeCreated ? "passed" : "failed")
                + " container_id=" + containerId
                + " recipe_display_id=" + WOODEN_PICKAXE_RECIPE_DISPLAY_ID
                + " planks_item=" + planksItemId
                + " planks_expected_count=" + expectedPlanksAfterPickaxe
                + " planks_count_matched=" + pickaxePlanksConsumed
                + " stick_expected_count=" + expectedStickAfterPickaxe
                + " stick_count_matched=" + pickaxeSticksConsumed
                + " wooden_pickaxe_expected_count=" + expectedWoodenPickaxeCount
                + " wooden_pickaxe_count_matched=" + pickaxeCreated
        );
        boolean closed = !closeScreen || client.closeCurrentScreen(INVENTORY_TIMEOUT);
        if (closeScreen) {
            observations.add("crafting table screen close after tool: " + (closed ? "passed" : "failed"));
        }
        if (!pickaxePlanksConsumed || !pickaxeSticksConsumed || !pickaxeCreated || !closed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport mineCobblestoneWithWoodenPickaxe(
        String id,
        List<String> observations,
        ScenarioClient client,
        int targetCobblestoneCount
    ) throws Exception {
        ScenarioHeldItem woodenPickaxe = client.selectHotbarItem("minecraft:wooden_pickaxe", 1, HOTBAR_TIMEOUT);
        if (!woodenPickaxe.matches("minecraft:wooden_pickaxe", 1)) {
            observations.add("blocked: crafted wooden pickaxe is not selectable for stone mining");
            return new ClientScenarioReport("blocked", id, observations);
        }

        int miningAttempts = 0;
        int maxMiningAttempts = targetCobblestoneCount + 6;
        while (
            client.inventoryCount("minecraft:cobblestone") < targetCobblestoneCount
                && miningAttempts < maxMiningAttempts
        ) {
            miningAttempts += 1;
            ScenarioBlockTarget stone = client.findBreakableBlock(
                List.of("minecraft:stone"),
                ScenarioReach.WITHIN_SURVIVAL_REACH
            );
            if (stone == null) {
                ScenarioBlockTarget farStone = client.findBreakableBlock(
                    List.of("minecraft:stone"),
                    ScenarioReach.OUTSIDE_SURVIVAL_REACH
                );
                if (farStone == null) {
                    observations.add("blocked: no loaded natural stone found near the real client");
                    return new ClientScenarioReport("blocked", id, observations);
                }
                boolean approached = client.approachBlock(farStone, APPROACH_TIMEOUT);
                observations.add(
                    "natural stone approach: " + (approached ? "passed" : "failed")
                        + " target=" + coordinates(farStone)
                );
                stone = client.findBreakableBlock(List.of("minecraft:stone"), ScenarioReach.WITHIN_SURVIVAL_REACH);
                if (!approached) {
                    if (stone == null) {
                        return new ClientScenarioReport("blocked", id, observations);
                    }
                    observations.add(
                        "natural stone reachable fallback after failed approach: passed"
                            + " target=" + coordinates(stone)
                    );
                } else if (stone == null) {
                    observations.add("blocked: natural stone remained outside survival reach after approach");
                    return new ClientScenarioReport("blocked", id, observations);
                }
            }

            boolean closeApproached = client.approachBlock(stone, APPROACH_TIMEOUT);
            observations.add(
                "natural stone close approach: " + (closeApproached ? "passed" : "failed")
                    + " target=" + coordinates(stone)
            );
            if (!closeApproached) {
                return new ClientScenarioReport("blocked", id, observations);
            }

            int cobblestoneBefore = client.inventoryCount("minecraft:cobblestone");
            ScenarioBreakResult broke = client.breakBlockUntilDropVisible(
                stone,
                "minecraft:cobblestone",
                BREAK_TIMEOUT
            );
            ScenarioBreakResult pickup = client.collectVisibleItemDrop(
                stone,
                "minecraft:cobblestone",
                1,
                PICKUP_TIMEOUT
            );
            int cobblestoneAfter = client.inventoryCount("minecraft:cobblestone");
            boolean inventoryAdvanced = cobblestoneAfter >= cobblestoneBefore + 1;
            boolean naturalPickup = broke.started()
                && broke.becameAir()
                && broke.sawDrop()
                && inventoryAdvanced;
            observations.add(
                "stone break/drop/pickup: " + (naturalPickup ? "passed" : "failed")
                    + " target=" + coordinates(stone)
                    + " break_started=" + broke.started()
                    + " became_air=" + broke.becameAir()
                    + " saw_drop=" + broke.sawDrop()
                    + " pickup_restored=" + pickup.pickupRestored()
                    + " cobblestone_before=" + cobblestoneBefore
                    + " cobblestone_after=" + cobblestoneAfter
                    + " held=" + pickup.selectedItem().itemId() + " x" + pickup.selectedItem().count()
            );
            if (!naturalPickup) {
                if (broke.started() && broke.becameAir() && broke.sawDrop() && !inventoryAdvanced) {
                    continue;
                }
                return new ClientScenarioReport("failed", id, observations);
            }
        }

        int cobblestoneCount = client.inventoryCount("minecraft:cobblestone");
        if (cobblestoneCount < targetCobblestoneCount) {
            observations.add(
                "stone inventory: failed cobblestone_count=" + cobblestoneCount
                    + " expected_at_least=" + targetCobblestoneCount
                    + " mining_attempts=" + miningAttempts
            );
            return new ClientScenarioReport("failed", id, observations);
        }
        observations.add(
            "stone inventory: passed cobblestone_count=" + cobblestoneCount
                + " expected_at_least=" + targetCobblestoneCount
                + " mining_attempts=" + miningAttempts
        );
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport craftStonePickaxeInOpenTable(
        String id,
        List<String> observations,
        ScenarioClient client,
        boolean closeScreen
    ) throws Exception {
        int containerId = client.activeContainerId();
        int cobblestoneCount = client.inventoryCount("minecraft:cobblestone");
        int stickCount = client.inventoryCount("minecraft:stick");
        int stonePickaxeCount = client.inventoryCount("minecraft:stone_pickaxe");
        if (cobblestoneCount < 3 || stickCount < 2) {
            observations.add("stone pickaxe recipe: failed missing cobblestone or sticks");
            return new ClientScenarioReport("failed", id, observations);
        }
        int expectedCobblestoneAfterPickaxe = cobblestoneCount - 3;
        int expectedStickAfterPickaxe = stickCount - 2;
        int expectedStonePickaxeCount = stonePickaxeCount + 1;
        client.placeRecipe(containerId, STONE_PICKAXE_RECIPE_DISPLAY_ID, false);
        boolean pickaxeCobblestoneConsumed = client.waitForInventoryCount(
            "minecraft:cobblestone",
            expectedCobblestoneAfterPickaxe,
            INVENTORY_TIMEOUT
        );
        boolean pickaxeSticksConsumed = client.waitForInventoryCount(
            "minecraft:stick",
            expectedStickAfterPickaxe,
            INVENTORY_TIMEOUT
        );
        boolean pickaxeCreated = client.waitForInventoryCount(
            "minecraft:stone_pickaxe",
            expectedStonePickaxeCount,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "stone pickaxe recipe: "
                + (pickaxeCobblestoneConsumed && pickaxeSticksConsumed && pickaxeCreated ? "passed" : "failed")
                + " container_id=" + containerId
                + " recipe_display_id=" + STONE_PICKAXE_RECIPE_DISPLAY_ID
                + " cobblestone_expected_count=" + expectedCobblestoneAfterPickaxe
                + " cobblestone_count_matched=" + pickaxeCobblestoneConsumed
                + " stick_expected_count=" + expectedStickAfterPickaxe
                + " stick_count_matched=" + pickaxeSticksConsumed
                + " stone_pickaxe_expected_count=" + expectedStonePickaxeCount
                + " stone_pickaxe_count_matched=" + pickaxeCreated
        );
        boolean closed = !closeScreen || client.closeCurrentScreen(INVENTORY_TIMEOUT);
        if (closeScreen) {
            observations.add("crafting table screen close after stone tool: " + (closed ? "passed" : "failed"));
        }
        if (!pickaxeCobblestoneConsumed || !pickaxeSticksConsumed || !pickaxeCreated || !closed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport craftStoneSwordInOpenTable(
        String id,
        List<String> observations,
        ScenarioClient client,
        boolean closeScreen
    ) throws Exception {
        int containerId = client.activeContainerId();
        int cobblestoneCount = client.inventoryCount("minecraft:cobblestone");
        int stickCount = client.inventoryCount("minecraft:stick");
        int stoneSwordCount = client.inventoryCount("minecraft:stone_sword");
        if (cobblestoneCount < 2 || stickCount < 1) {
            observations.add("stone sword recipe: failed missing cobblestone or sticks");
            return new ClientScenarioReport("failed", id, observations);
        }
        int expectedCobblestoneAfterSword = cobblestoneCount - 2;
        int expectedStickAfterSword = stickCount - 1;
        int expectedStoneSwordCount = stoneSwordCount + 1;
        client.placeRecipe(containerId, STONE_SWORD_RECIPE_DISPLAY_ID, false);
        boolean swordCobblestoneConsumed = client.waitForInventoryCount(
            "minecraft:cobblestone",
            expectedCobblestoneAfterSword,
            INVENTORY_TIMEOUT
        );
        boolean swordSticksConsumed = client.waitForInventoryCount(
            "minecraft:stick",
            expectedStickAfterSword,
            INVENTORY_TIMEOUT
        );
        boolean swordCreated = client.waitForInventoryCount(
            "minecraft:stone_sword",
            expectedStoneSwordCount,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "stone sword recipe: "
                + (swordCobblestoneConsumed && swordSticksConsumed && swordCreated ? "passed" : "failed")
                + " container_id=" + containerId
                + " recipe_display_id=" + STONE_SWORD_RECIPE_DISPLAY_ID
                + " cobblestone_expected_count=" + expectedCobblestoneAfterSword
                + " cobblestone_count_matched=" + swordCobblestoneConsumed
                + " stick_expected_count=" + expectedStickAfterSword
                + " stick_count_matched=" + swordSticksConsumed
                + " stone_sword_expected_count=" + expectedStoneSwordCount
                + " stone_sword_count_matched=" + swordCreated
        );
        boolean closed = !closeScreen || client.closeCurrentScreen(INVENTORY_TIMEOUT);
        if (closeScreen) {
            observations.add("crafting table screen close after stone sword: " + (closed ? "passed" : "failed"));
        }
        if (!swordCobblestoneConsumed || !swordSticksConsumed || !swordCreated || !closed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport craftIronSwordInOpenTable(
        String id,
        List<String> observations,
        ScenarioClient client,
        String planksItemId,
        boolean closeScreen
    ) throws Exception {
        int containerId = client.activeContainerId();
        int stickCount = client.inventoryCount("minecraft:stick");
        if (stickCount < 1) {
            int planksCount = client.inventoryCount(planksItemId);
            if (planksCount < 2) {
                observations.add("iron sword stick recipe: failed fewer than two planks available");
                return new ClientScenarioReport("failed", id, observations);
            }
            int expectedPlanksAfterSticks = planksCount - 2;
            int expectedStickCount = stickCount + 4;
            client.placeRecipe(containerId, STICK_RECIPE_DISPLAY_ID, false);
            boolean stickPlanksConsumed = client.waitForInventoryCount(
                planksItemId,
                expectedPlanksAfterSticks,
                INVENTORY_TIMEOUT
            );
            boolean sticksCreated = client.waitForInventoryCount(
                "minecraft:stick",
                expectedStickCount,
                INVENTORY_TIMEOUT
            );
            observations.add(
                "iron sword stick recipe: "
                    + (stickPlanksConsumed && sticksCreated ? "passed" : "failed")
                    + " container_id=" + containerId
                    + " recipe_display_id=" + STICK_RECIPE_DISPLAY_ID
                    + " planks_item=" + planksItemId
                    + " planks_expected_count=" + expectedPlanksAfterSticks
                    + " planks_count_matched=" + stickPlanksConsumed
                    + " stick_expected_count=" + expectedStickCount
                    + " stick_count_matched=" + sticksCreated
            );
            if (!stickPlanksConsumed || !sticksCreated) {
                return new ClientScenarioReport("failed", id, observations);
            }
        }

        int ironIngotCount = client.inventoryCount("minecraft:iron_ingot");
        stickCount = client.inventoryCount("minecraft:stick");
        int ironSwordCount = client.inventoryCount("minecraft:iron_sword");
        if (ironIngotCount < 2 || stickCount < 1) {
            observations.add("iron sword recipe: failed missing iron ingots or sticks");
            return new ClientScenarioReport("failed", id, observations);
        }
        int expectedIronIngotAfterSword = ironIngotCount - 2;
        int expectedStickAfterSword = stickCount - 1;
        int expectedIronSwordCount = ironSwordCount + 1;
        client.placeRecipe(containerId, IRON_SWORD_RECIPE_DISPLAY_ID, false);
        boolean swordIronConsumed = client.waitForInventoryCount(
            "minecraft:iron_ingot",
            expectedIronIngotAfterSword,
            INVENTORY_TIMEOUT
        );
        boolean swordStickConsumed = client.waitForInventoryCount(
            "minecraft:stick",
            expectedStickAfterSword,
            INVENTORY_TIMEOUT
        );
        boolean swordCreated = client.waitForInventoryCount(
            "minecraft:iron_sword",
            expectedIronSwordCount,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "iron sword recipe: "
                + (swordIronConsumed && swordStickConsumed && swordCreated ? "passed" : "failed")
                + " container_id=" + containerId
                + " recipe_display_id=" + IRON_SWORD_RECIPE_DISPLAY_ID
                + " iron_ingot_expected_count=" + expectedIronIngotAfterSword
                + " iron_ingot_count_matched=" + swordIronConsumed
                + " stick_expected_count=" + expectedStickAfterSword
                + " stick_count_matched=" + swordStickConsumed
                + " iron_sword_expected_count=" + expectedIronSwordCount
                + " iron_sword_count_matched=" + swordCreated
        );
        boolean closed = !closeScreen || client.closeCurrentScreen(INVENTORY_TIMEOUT);
        if (closeScreen) {
            observations.add("crafting table screen close after iron sword: " + (closed ? "passed" : "failed"));
        }
        if (!swordIronConsumed || !swordStickConsumed || !swordCreated || !closed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport craftShieldInOpenTable(
        String id,
        List<String> observations,
        ScenarioClient client,
        String planksItemId,
        boolean closeScreen
    ) throws Exception {
        int containerId = client.activeContainerId();
        int planksCount = client.inventoryCount(planksItemId);
        int ironIngotCount = client.inventoryCount("minecraft:iron_ingot");
        int shieldCount = client.inventoryCount("minecraft:shield");
        if (planksCount < 6 || ironIngotCount < 1) {
            observations.add("shield recipe: failed missing planks or iron ingot");
            return new ClientScenarioReport("failed", id, observations);
        }

        int expectedPlanksAfterShield = planksCount - 6;
        int expectedIronIngotAfterShield = ironIngotCount - 1;
        int expectedShieldCount = shieldCount + 1;
        client.placeRecipe(containerId, SHIELD_RECIPE_DISPLAY_ID, false);
        boolean shieldPlanksConsumed = client.waitForInventoryCount(
            planksItemId,
            expectedPlanksAfterShield,
            INVENTORY_TIMEOUT
        );
        boolean shieldIronConsumed = client.waitForInventoryCount(
            "minecraft:iron_ingot",
            expectedIronIngotAfterShield,
            INVENTORY_TIMEOUT
        );
        boolean shieldCreated = client.waitForInventoryCount(
            "minecraft:shield",
            expectedShieldCount,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "shield recipe: "
                + (shieldPlanksConsumed && shieldIronConsumed && shieldCreated ? "passed" : "failed")
                + " container_id=" + containerId
                + " recipe_display_id=" + SHIELD_RECIPE_DISPLAY_ID
                + " planks_item=" + planksItemId
                + " planks_expected_count=" + expectedPlanksAfterShield
                + " planks_count_matched=" + shieldPlanksConsumed
                + " iron_ingot_expected_count=" + expectedIronIngotAfterShield
                + " iron_ingot_count_matched=" + shieldIronConsumed
                + " shield_expected_count=" + expectedShieldCount
                + " shield_count_matched=" + shieldCreated
        );
        boolean closed = !closeScreen || client.closeCurrentScreen(INVENTORY_TIMEOUT);
        if (closeScreen) {
            observations.add("crafting table screen close after shield: " + (closed ? "passed" : "failed"));
        }
        if (!shieldPlanksConsumed || !shieldIronConsumed || !shieldCreated || !closed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport craftIronChestplateInOpenTable(
        String id,
        List<String> observations,
        ScenarioClient client,
        boolean closeScreen
    ) throws Exception {
        int containerId = client.activeContainerId();
        int ironIngotCount = client.inventoryCount("minecraft:iron_ingot");
        int ironChestplateCount = client.inventoryCount("minecraft:iron_chestplate");
        if (ironIngotCount < 8) {
            observations.add("iron chestplate recipe: failed missing iron ingots");
            return new ClientScenarioReport("failed", id, observations);
        }

        int expectedIronIngotAfterChestplate = ironIngotCount - 8;
        int expectedIronChestplateCount = ironChestplateCount + 1;
        client.placeRecipe(containerId, IRON_CHESTPLATE_RECIPE_DISPLAY_ID, false);
        boolean chestplateIronConsumed = client.waitForInventoryCount(
            "minecraft:iron_ingot",
            expectedIronIngotAfterChestplate,
            INVENTORY_TIMEOUT
        );
        boolean chestplateCreated = client.waitForInventoryCount(
            "minecraft:iron_chestplate",
            expectedIronChestplateCount,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "iron chestplate recipe: "
                + (chestplateIronConsumed && chestplateCreated ? "passed" : "failed")
                + " container_id=" + containerId
                + " recipe_display_id=" + IRON_CHESTPLATE_RECIPE_DISPLAY_ID
                + " iron_ingot_expected_count=" + expectedIronIngotAfterChestplate
                + " iron_ingot_count_matched=" + chestplateIronConsumed
                + " iron_chestplate_expected_count=" + expectedIronChestplateCount
                + " iron_chestplate_count_matched=" + chestplateCreated
        );
        boolean closed = !closeScreen || client.closeCurrentScreen(INVENTORY_TIMEOUT);
        if (closeScreen) {
            observations.add("crafting table screen close after iron chestplate: " + (closed ? "passed" : "failed"));
        }
        if (!chestplateIronConsumed || !chestplateCreated || !closed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport craftFurnaceInOpenTable(
        String id,
        List<String> observations,
        ScenarioClient client,
        boolean closeScreen
    ) throws Exception {
        int containerId = client.activeContainerId();
        int cobblestoneCount = client.inventoryCount("minecraft:cobblestone");
        int furnaceCount = client.inventoryCount("minecraft:furnace");
        if (cobblestoneCount < 8) {
            observations.add("furnace recipe: failed fewer than eight cobblestone available");
            return new ClientScenarioReport("failed", id, observations);
        }
        int expectedCobblestoneAfterFurnace = cobblestoneCount - 8;
        int expectedFurnaceCount = furnaceCount + 1;
        client.placeRecipe(containerId, FURNACE_RECIPE_DISPLAY_ID, false);
        boolean furnaceCobblestoneConsumed = client.waitForInventoryCount(
            "minecraft:cobblestone",
            expectedCobblestoneAfterFurnace,
            INVENTORY_TIMEOUT
        );
        boolean furnaceCreated = client.waitForInventoryCount(
            "minecraft:furnace",
            expectedFurnaceCount,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "furnace recipe: "
                + (furnaceCobblestoneConsumed && furnaceCreated ? "passed" : "failed")
                + " container_id=" + containerId
                + " recipe_display_id=" + FURNACE_RECIPE_DISPLAY_ID
                + " cobblestone_expected_count=" + expectedCobblestoneAfterFurnace
                + " cobblestone_count_matched=" + furnaceCobblestoneConsumed
                + " furnace_expected_count=" + expectedFurnaceCount
                + " furnace_count_matched=" + furnaceCreated
        );
        boolean closed = !closeScreen || client.closeCurrentScreen(INVENTORY_TIMEOUT);
        if (closeScreen) {
            observations.add("crafting table screen close after furnace: " + (closed ? "passed" : "failed"));
        }
        if (!furnaceCobblestoneConsumed || !furnaceCreated || !closed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        return new ClientScenarioReport("passed", id, observations);
    }

    private FurnacePlacementOpenResult placeAndOpenFurnace(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        ScenarioHeldItem furnace = client.selectHotbarItem("minecraft:furnace", 1, HOTBAR_TIMEOUT);
        if (!furnace.matches("minecraft:furnace", 1)) {
            observations.add("blocked: crafted furnace exists but is not selectable from hotbar");
            return new FurnacePlacementOpenResult(new ClientScenarioReport("blocked", id, observations), null, null);
        }
        ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (pair == null) {
            observations.add("blocked: no loaded dry target found for furnace placement");
            return new FurnacePlacementOpenResult(new ClientScenarioReport("blocked", id, observations), null, null);
        }
        ScenarioBlockTarget clicked = new ScenarioBlockTarget(
            pair.clicked().x(),
            pair.clicked().y(),
            pair.clicked().z(),
            pair.clicked().face(),
            "furnace-clicked",
            pair.clicked().blockId()
        );
        ScenarioBlockTarget target = new ScenarioBlockTarget(
            pair.target().x(),
            pair.target().y(),
            pair.target().z(),
            "up",
            "furnace-target",
            "minecraft:furnace"
        );
        ScenarioUseResult placeUse = client.useItemOn(clicked, furnace);
        boolean placed = client.waitForBlock(target, "minecraft:furnace", BLOCK_TIMEOUT);
        ScenarioBlockTarget furnaceUseTarget = reachableUseTarget(client, target);
        ScenarioUseResult openUse = client.useItemOn(furnaceUseTarget, furnace);
        boolean opened = client.waitForScreenClassName(FURNACE_SCREEN, INVENTORY_TIMEOUT);
        boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        boolean passed = placed && opened && closed;
        observations.add(
            "furnace open: " + (passed ? "passed" : "failed")
                + " place_use_result=" + placeUse.result()
                + " placed=" + placed
                + " open_use_result=" + openUse.result()
                + " screen=" + FURNACE_SCREEN
                + " screen_matched=" + opened
                + " closed=" + closed
        );
        if (!passed) {
            return new FurnacePlacementOpenResult(new ClientScenarioReport("failed", id, observations), null, target);
        }
        return new FurnacePlacementOpenResult(new ClientScenarioReport("passed", id, observations), null, target);
    }

    private ClientScenarioReport runTwentyMinuteSurvivalLoop(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        ClientScenarioReport beforeRestart = runSaveRestartBefore(id, observations, screenshotsDir, client);
        if (!"passed".equals(beforeRestart.result())) {
            return beforeRestart;
        }

        long soakMillis = survivalSoakDuration.toMillis();
        long soakTicks = Math.max(1L, (soakMillis + 49L) / 50L);
        observations.add("20-minute survival soak: started duration_millis=" + soakMillis);
        ScenarioHeldItem weapon = client.selectHotbarItem("minecraft:wooden_pickaxe", 1, HOTBAR_TIMEOUT);
        if (!weapon.matches("minecraft:wooden_pickaxe", 1)) {
            observations.add("20-minute survival soak: failed wooden_pickaxe_selectable=false");
            return new ClientScenarioReport("failed", id, observations);
        }

        long startServerTime = client.serverGameTime();
        if (startServerTime == Long.MIN_VALUE) {
            startServerTime = client.waitForServerTimeAfter(Long.MIN_VALUE, SURVIVAL_SOAK_STEP_TIMEOUT);
        }
        if (startServerTime == Long.MIN_VALUE) {
            observations.add("20-minute survival soak: failed initial_server_time_packet=false");
            return new ClientScenarioReport("failed", id, observations);
        }
        long currentServerTime = startServerTime;
        long completedTicks = 0L;
        long nextResourceTick = 0L;
        int consecutiveResourceBlocks = 0;
        int resourceRuns = 0;
        int hostilesNeutralized = 0;
        while (completedTicks < soakTicks) {
            long nextServerTime = client.waitForServerTimeAfter(
                currentServerTime,
                SURVIVAL_SOAK_STEP_TIMEOUT
            );
            float health = client.playerHealth();
            if (health <= 0.0F) {
                observations.add(
                    "20-minute survival soak: failed player_died=true completed_ticks=" + completedTicks
                );
                return new ClientScenarioReport("failed", id, observations);
            }
            if (nextServerTime <= currentServerTime) {
                observations.add(
                    "20-minute survival soak: failed server_time_progress=false completed_ticks="
                        + completedTicks
                );
                return new ClientScenarioReport("failed", id, observations);
            }
            currentServerTime = nextServerTime;
            completedTicks = currentServerTime - startServerTime;

            ScenarioEntityObservation hostile = client.visibleEntity(
                HOSTILE_ENTITY_IDS,
                ScenarioReach.WITHIN_SURVIVAL_REACH
            );
            if (hostile != null) {
                boolean approached = client.approachEntity(hostile, APPROACH_TIMEOUT);
                boolean gone = approached && client.attackEntityUntilRemoved(hostile, ENTITY_ATTACK_TIMEOUT);
                float healthAfterCombat = client.playerHealth();
                observations.add(
                    "survival hostile defense: " + (gone && healthAfterCombat > 0.0F ? "passed" : "failed")
                        + " entity=" + hostile.entityType()
                        + " entity_id=" + hostile.entityId()
                        + " approached=" + approached
                        + " entity_gone=" + gone
                        + " health_after=" + healthAfterCombat
                );
                if (!gone || healthAfterCombat <= 0.0F) {
                    return new ClientScenarioReport("failed", id, observations);
                }
                hostilesNeutralized++;
                continue;
            }

            if (completedTicks < nextResourceTick) {
                continue;
            }

            String planksItemId = availablePlanksItem(client);
            String workItemId = weapon.itemId();
            if (planksItemId != null) {
                ScenarioHeldItem workItem = client.selectHotbarItem(planksItemId, 1, HOTBAR_TIMEOUT);
                if (!workItem.matches(planksItemId, 1)) {
                    observations.add("survival resource work: failed planks_selectable=false item=" + planksItemId);
                    return new ClientScenarioReport("failed", id, observations);
                }
                workItemId = planksItemId;
            }

            List<String> resourceObservations = new ArrayList<>();
            LogToPlanksResult resourceWork = runLogToPlanks(
                id,
                resourceObservations,
                client,
                1,
                false,
                false
            );
            if ("passed".equals(resourceWork.report().result())) {
                resourceRuns++;
                consecutiveResourceBlocks = 0;
                nextResourceTick = completedTicks + SURVIVAL_RESOURCE_INTERVAL_TICKS;
                observations.add(
                    "survival resource work: passed run=" + resourceRuns
                        + " completed_ticks=" + completedTicks
                        + " work_item=" + workItemId
                );
            } else if ("blocked".equals(resourceWork.report().result())) {
                consecutiveResourceBlocks++;
                nextResourceTick = completedTicks + SURVIVAL_RESOURCE_RETRY_TICKS;
                observations.add(
                    "survival resource work: blocked completed_ticks=" + completedTicks
                        + " consecutive=" + consecutiveResourceBlocks
                );
                if (consecutiveResourceBlocks >= MAX_CONSECUTIVE_RESOURCE_BLOCKS) {
                    observations.add("survival resource work: failed no_loaded_reachable_log=true");
                    return new ClientScenarioReport("failed", id, observations);
                }
            } else {
                observations.addAll(resourceObservations);
                observations.add("survival resource work: failed completed_ticks=" + completedTicks);
                return new ClientScenarioReport("failed", id, observations);
            }
            weapon = client.selectHotbarItem("minecraft:wooden_pickaxe", 1, HOTBAR_TIMEOUT);
            if (!weapon.matches("minecraft:wooden_pickaxe", 1)) {
                observations.add("20-minute survival soak: failed wooden_pickaxe_selectable_after_work=false");
                return new ClientScenarioReport("failed", id, observations);
            }
        }

        ScenarioHeldItem selected = client.selectedItem();
        int woodenPickaxeCount = client.inventoryCount("minecraft:wooden_pickaxe");
        boolean inventoryStillPresent = woodenPickaxeCount >= 1;
        observations.add(
            "20-minute survival soak: " + (inventoryStillPresent ? "passed" : "failed")
                + " duration_millis=" + soakMillis
                + " ticks=" + soakTicks
                + " resource_runs=" + resourceRuns
                + " hostiles_neutralized=" + hostilesNeutralized
                + " selected=" + selected.itemId() + " x" + selected.count()
                + " wooden_pickaxe_count=" + woodenPickaxeCount
        );
        observations.add("runner-managed restart: pending clean server restart and post-restart rejoin check");
        return new ClientScenarioReport(inventoryStillPresent ? "passed" : "failed", id, observations);
    }

    private static String availablePlanksItem(ScenarioClient client) throws Exception {
        for (PlanksRecipe recipe : PLANKS_BY_LOG.values()) {
            if (client.inventoryCount(recipe.planksItemId()) > 0) {
                return recipe.planksItemId();
            }
        }
        return null;
    }

    private ClientScenarioReport runSaveRestartBefore(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        LogToPlanksResult planks = runLogToPlanks(id, observations, client, 3, true, false);
        if (!"passed".equals(planks.report().result())) {
            return planks.report();
        }
        CraftingTableOpenResult table = craftPlaceAndOpenTable(
            id,
            observations,
            client,
            planks.planks().planksItemId(),
            false
        );
        if (!"passed".equals(table.report().result())) {
            return table.report();
        }
        ClientScenarioReport tool = craftWoodenPickaxeInOpenTable(
            id,
            observations,
            client,
            planks.planks().planksItemId(),
            true
        );
        if (!"passed".equals(tool.result())) {
            return tool;
        }

        writeMarker(saveRestartMarkerPath(screenshotsDir), table.tableTarget());
        observations.add("restart marker placement: passed target=" + coordinates(table.tableTarget()));
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runSaveRestartAfter(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = saveRestartMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing restart marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioBlockTarget marker = readMarker(markerPath, "restart-marker");
        boolean markerPersisted = client.waitForBlock(marker, "minecraft:crafting_table", BLOCK_TIMEOUT);
        int woodenPickaxeCount = client.inventoryCount("minecraft:wooden_pickaxe");
        boolean inventoryPersisted = woodenPickaxeCount >= 1;
        observations.add(
            "restart marker persistence: " + (markerPersisted ? "passed" : "failed")
                + " target=" + coordinates(marker)
        );
        observations.add(
            "inventory persistence: " + (inventoryPersisted ? "passed" : "failed")
                + " wooden_pickaxe_count=" + woodenPickaxeCount
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(
            markerPersisted && inventoryPersisted ? "passed" : "failed",
            id,
            observations
        );
    }

    private ClientScenarioReport runStoneToolSaveRestartBefore(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        StoneToolProgressionResult stoneTool = runStoneToolProgression(id, observations, client);
        if (!"passed".equals(stoneTool.report().result())) {
            return stoneTool.report();
        }
        if (stoneTool.tableTarget() == null) {
            observations.add("restart marker placement: failed missing crafted table target");
            return new ClientScenarioReport("failed", id, observations);
        }

        writeMarker(saveRestartMarkerPath(screenshotsDir), stoneTool.tableTarget());
        observations.add("restart marker placement: passed target=" + coordinates(stoneTool.tableTarget()));
        observations.add("runner-managed restart: pending clean server restart and stone-tool rejoin check");
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runStoneToolSaveRestartAfter(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = saveRestartMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing restart marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioBlockTarget marker = readMarker(markerPath, "restart-marker");
        boolean markerPersisted = client.waitForBlock(marker, "minecraft:crafting_table", BLOCK_TIMEOUT);
        int stonePickaxeCount = client.inventoryCount("minecraft:stone_pickaxe");
        boolean inventoryPersisted = stonePickaxeCount >= 1;
        observations.add(
            "restart marker persistence: " + (markerPersisted ? "passed" : "failed")
                + " target=" + coordinates(marker)
        );
        observations.add(
            "stone inventory persistence: " + (inventoryPersisted ? "passed" : "failed")
                + " stone_pickaxe_count=" + stonePickaxeCount
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(
            markerPersisted && inventoryPersisted ? "passed" : "failed",
            id,
            observations
        );
    }

    private ClientScenarioReport runIronSwordSaveRestartBefore(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        IronSwordProgressionResult ironSword = craftEarnedIronSwordProgression(id, observations, client);
        if (!"passed".equals(ironSword.report().result())) {
            return ironSword.report();
        }
        if (ironSword.tableTarget() == null) {
            observations.add("restart marker placement: failed missing crafted table target");
            return new ClientScenarioReport("failed", id, observations);
        }

        writeMarker(saveRestartMarkerPath(screenshotsDir), ironSword.tableTarget());
        observations.add("restart marker placement: passed target=" + coordinates(ironSword.tableTarget()));
        observations.add("runner-managed restart: pending clean server restart and iron-sword rejoin check");
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runIronSwordSaveRestartAfter(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = saveRestartMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing restart marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioBlockTarget marker = readMarker(markerPath, "restart-marker");
        boolean markerPersisted = client.waitForBlock(marker, "minecraft:crafting_table", BLOCK_TIMEOUT);
        int ironSwordCount = client.inventoryCount("minecraft:iron_sword");
        boolean inventoryPersisted = ironSwordCount >= 1;
        observations.add(
            "restart marker persistence: " + (markerPersisted ? "passed" : "failed")
                + " target=" + coordinates(marker)
        );
        observations.add(
            "iron sword inventory persistence: " + (inventoryPersisted ? "passed" : "failed")
                + " iron_sword_count=" + ironSwordCount
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(
            markerPersisted && inventoryPersisted ? "passed" : "failed",
            id,
            observations
        );
    }

    private ClientScenarioReport runIronChestplateSaveRestartMitigationBefore(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        IronChestplateProgressionResult chestplate = craftEarnedIronChestplateProgressionResult(
            id,
            observations,
            client
        );
        if (!"passed".equals(chestplate.report().result())) {
            return chestplate.report();
        }
        if (chestplate.tableTarget() == null) {
            observations.add("restart marker placement: failed missing crafted table target");
            return new ClientScenarioReport("failed", id, observations);
        }

        ClientScenarioReport equipped = equipEarnedIronChestplate(id, observations, client);
        if (!"passed".equals(equipped.result())) {
            return equipped;
        }

        writeMarker(saveRestartMarkerPath(screenshotsDir), chestplate.tableTarget());
        observations.add("restart marker placement: passed target=" + coordinates(chestplate.tableTarget()));
        observations.add("runner-managed restart: pending clean server restart and iron-chestplate rejoin check");
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runIronChestplateSaveRestartMitigationAfter(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = saveRestartMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing restart marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioBlockTarget marker = readMarker(markerPath, "restart-marker");
        boolean markerPersisted = client.waitForBlock(marker, "minecraft:crafting_table", BLOCK_TIMEOUT);
        ScenarioHeldItem chestArmor = client.equippedArmor("chest");
        boolean armorPersisted = chestArmor.matches("minecraft:iron_chestplate", 1);
        observations.add(
            "restart marker persistence: " + (markerPersisted ? "passed" : "failed")
                + " target=" + coordinates(marker)
        );
        observations.add(
            "iron chestplate armor persistence: " + (armorPersisted ? "passed" : "failed")
                + " armor_slot=chest"
                + " item=" + chestArmor.itemId()
                + " count=" + chestArmor.count()
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        if (!markerPersisted || !armorPersisted) {
            return new ClientScenarioReport("failed", id, observations);
        }

        return measureIronChestplateZombieMitigation(
            id,
            observations,
            client,
            "iron chestplate restarted zombie mitigation"
        );
    }

    private ClientScenarioReport runTwoClientSharedLogDropBreak(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        ScenarioBlockTarget log = findReachableNaturalLog(id, observations, client, "two-client shared log");
        if (log == null) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        boolean closeApproached = "down".equals(log.face()) || client.approachBlock(log, APPROACH_TIMEOUT);
        observations.add(
            "two-client shared log close approach: " + (closeApproached ? "passed" : "failed")
                + " target=" + coordinates(log)
        );
        if (!closeApproached) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        ScenarioBreakResult broke = client.breakBlockUntilDropVisible(log, log.blockId(), BREAK_TIMEOUT);
        boolean dropped = broke.started() && broke.becameAir() && broke.sawDrop();
        observations.add(
            "two-client shared log drop break: " + (dropped ? "passed" : "failed")
                + " target=" + coordinates(log)
                + " item=" + log.blockId()
                + " break_started=" + broke.started()
                + " became_air=" + broke.becameAir()
                + " saw_drop=" + broke.sawDrop()
        );
        if (!dropped) {
            return new ClientScenarioReport("failed", id, observations);
        }

        writeSharedLogDropMarker(sharedLogDropMarkerPath(screenshotsDir), log, log.blockId());
        observations.add("shared log drop marker: passed target=" + coordinates(log) + " item=" + log.blockId());
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport("passed", id, observations);
    }

    private ScenarioBlockTarget findReachableNaturalLog(
        String id,
        List<String> observations,
        ScenarioClient client,
        String label
    ) throws Exception {
        ScenarioBlockTarget log = client.findBreakableBlock(
            SUPPORTED_LOG_BLOCK_IDS,
            ScenarioReach.WITHIN_SURVIVAL_REACH
        );
        if (log != null) {
            return log;
        }

        ScenarioBlockTarget farLog = client.findBreakableBlock(
            SUPPORTED_LOG_BLOCK_IDS,
            ScenarioReach.OUTSIDE_SURVIVAL_REACH
        );
        if (farLog == null) {
            observations.add("blocked: no loaded supported natural log found for " + id);
            return null;
        }
        boolean approached = client.approachBlock(farLog, APPROACH_TIMEOUT);
        observations.add(
            label + " approach: " + (approached ? "passed" : "failed")
                + " target=" + coordinates(farLog)
        );
        log = client.findBreakableBlock(SUPPORTED_LOG_BLOCK_IDS, ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (!approached && log == null) {
            return null;
        }
        if (log == null) {
            observations.add("blocked: supported natural log remained outside survival reach after approach");
            return null;
        }
        return log;
    }

    private ClientScenarioReport runTwoClientSharedLogDropObserve(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = sharedLogDropMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing shared log drop marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        SharedLogDropMarker marker = readSharedLogDropMarker(markerPath);
        boolean visible = client.waitForVisibleItemDrop(marker.itemId(), marker.target(), PICKUP_TIMEOUT);
        observations.add(
            "two-client shared log drop visibility: " + (visible ? "passed" : "failed")
                + " target=" + coordinates(marker.target())
                + " item=" + marker.itemId()
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(visible ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientSharedLogPickupCollect(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = sharedLogDropMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing shared log drop marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        SharedLogDropMarker marker = readSharedLogDropMarker(markerPath);
        ScenarioBreakResult pickup = client.collectVisibleItemDrop(
            marker.target(),
            marker.itemId(),
            1,
            PICKUP_TIMEOUT
        );
        boolean collected = pickup.becameAir() && pickup.pickupRestored();
        observations.add(
            "two-client shared log pickup: " + (collected ? "passed" : "failed")
                + " target=" + coordinates(marker.target())
                + " item=" + marker.itemId()
                + " saw_drop=" + pickup.sawDrop()
                + " drop_gone=" + pickup.becameAir()
                + " pickup_restored=" + pickup.pickupRestored()
                + " held=" + pickup.selectedItem().itemId() + " x" + pickup.selectedItem().count()
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(collected ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientSharedLogPickupGoneObserve(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = sharedLogDropMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing shared log drop marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        SharedLogDropMarker marker = readSharedLogDropMarker(markerPath);
        boolean removed = client.waitForNoVisibleItemDrop(marker.itemId(), marker.target(), PICKUP_TIMEOUT);
        observations.add(
            "two-client shared log pickup removal: " + (removed ? "passed" : "failed")
                + " target=" + coordinates(marker.target())
                + " item=" + marker.itemId()
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(removed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientEarnedSharedChestDeposit(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        LogToPlanksResult planks = runLogToPlanks(id, observations, client, 4, true, false);
        if (!"passed".equals(planks.report().result())) {
            return planks.report();
        }
        CraftingTableOpenResult table = craftPlaceAndOpenTable(
            id,
            observations,
            client,
            planks.planks().planksItemId(),
            false
        );
        if (!"passed".equals(table.report().result())) {
            return table.report();
        }

        int planksCount = client.inventoryCount(planks.planks().planksItemId());
        int chestCount = client.inventoryCount("minecraft:chest");
        if (planksCount < 8) {
            observations.add("two-client shared chest recipe: failed fewer than eight earned planks available");
            return new ClientScenarioReport("failed", id, observations);
        }
        int expectedPlanksAfterChest = planksCount - 8;
        int expectedChestCount = chestCount + 1;
        int containerId = client.activeContainerId();
        client.placeRecipe(containerId, CHEST_RECIPE_DISPLAY_ID, false);
        boolean planksConsumed = client.waitForInventoryCount(
            planks.planks().planksItemId(),
            expectedPlanksAfterChest,
            INVENTORY_TIMEOUT
        );
        boolean chestCreated = client.waitForInventoryCount(
            "minecraft:chest",
            expectedChestCount,
            INVENTORY_TIMEOUT
        );
        observations.add(
            "two-client shared chest recipe: " + (planksConsumed && chestCreated ? "passed" : "failed")
                + " container_id=" + containerId
                + " recipe_display_id=" + CHEST_RECIPE_DISPLAY_ID
                + " planks_item=" + planks.planks().planksItemId()
                + " planks_expected_count=" + expectedPlanksAfterChest
                + " chest_expected_count=" + expectedChestCount
        );
        if (!planksConsumed || !chestCreated) {
            return new ClientScenarioReport("failed", id, observations);
        }
        boolean craftingClosed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        observations.add(
            "two-client shared chest crafting screen close: "
                + (craftingClosed ? "passed" : "failed")
        );
        if (!craftingClosed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioHeldItem chest = client.selectHotbarItem("minecraft:chest", 1, HOTBAR_TIMEOUT);
        if (!chest.matches("minecraft:chest", 1)) {
            observations.add("blocked: earned shared chest exists but is not selectable from hotbar");
            return new ClientScenarioReport("blocked", id, observations);
        }
        ScenarioBlockPair pair = client.findDryPlaceablePair(ScenarioReach.WITHIN_SURVIVAL_REACH);
        if (pair == null) {
            observations.add("blocked: no loaded dry target found for shared chest placement");
            return new ClientScenarioReport("blocked", id, observations);
        }
        ScenarioUseResult placeUse = client.useItemOn(pair.clicked(), chest);
        boolean placed = client.waitForBlock(pair.target(), "minecraft:chest", BLOCK_TIMEOUT);
        ScenarioBlockTarget chestTarget = new ScenarioBlockTarget(
            pair.target().x(),
            pair.target().y(),
            pair.target().z(),
            "up",
            "playable-two-client-chest-marker",
            "minecraft:chest"
        );

        ScenarioHeldItem storedItem = client.selectHotbarItem(
            planks.planks().planksItemId(),
            1,
            HOTBAR_TIMEOUT
        );
        if (!storedItem.matches(planks.planks().planksItemId(), 1)) {
            observations.add("blocked: earned shared chest storage plank is not selectable from hotbar");
            return new ClientScenarioReport("blocked", id, observations);
        }

        ScenarioUseResult openUse = client.useItemOn(chestTarget, storedItem);
        boolean opened = client.waitForScreenClassName(CONTAINER_SCREEN, INVENTORY_TIMEOUT);
        observations.add(
            "two-client shared chest placement/open: " + (placed && opened ? "passed" : "failed")
                + " place_use_result=" + placeUse.result()
                + " placed=" + placed
                + " open_use_result=" + openUse.result()
                + " screen_matched=" + opened
                + " target=" + coordinates(chestTarget)
        );
        if (!placed || !opened) {
            return new ClientScenarioReport("failed", id, observations);
        }

        boolean deposited = client.moveSelectedItemToContainerSlot(
            0,
            planks.planks().planksItemId(),
            1,
            INVENTORY_TIMEOUT
        );
        boolean slotMatched = deposited
            && client.waitForContainerSlot(0, planks.planks().planksItemId(), 1, INVENTORY_TIMEOUT);
        boolean closed = client.closeCurrentScreen(INVENTORY_TIMEOUT);
        boolean passed = deposited && slotMatched && closed;
        observations.add(
            "two-client shared chest deposit: " + (passed ? "passed" : "failed")
                + " slot=0 item=" + planks.planks().planksItemId()
                + " moved=" + deposited
                + " slot_matched=" + slotMatched
                + " closed=" + closed
        );
        if (!passed) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ChestStorageResult stored = new ChestStorageResult(
            new ClientScenarioReport("passed", id, observations),
            chestTarget,
            planks.planks().planksItemId(),
            1
        );
        writeChestStorageMarker(sharedChestMarkerPath(screenshotsDir), stored);
        observations.add(
            "two-client shared chest marker: passed target=" + coordinates(stored.chestTarget())
                + " item=" + stored.itemId()
                + " count=" + stored.count()
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runTwoClientEarnedSharedChestWithdraw(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = sharedChestMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing shared chest marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        ChestStorageMarker marker = readSharedChestMarker(markerPath);
        boolean approached = client.approachBlock(marker.chestTarget(), APPROACH_TIMEOUT);
        observations.add(
            "two-client shared chest approach: " + (approached ? "passed" : "failed")
                + " target=" + coordinates(marker.chestTarget())
        );
        boolean visible = approached
            && client.waitForBlock(marker.chestTarget(), "minecraft:chest", BLOCK_TIMEOUT);
        ScenarioUseResult openUse = new ScenarioUseResult("skipped");
        boolean opened = false;
        boolean slotMatched = false;
        boolean moved = false;
        boolean empty = false;
        boolean closed = false;
        if (visible) {
            openUse = client.useItemOn(marker.chestTarget(), client.selectedItem());
            opened = client.waitForScreenClassName(CONTAINER_SCREEN, INVENTORY_TIMEOUT);
            slotMatched = opened
                && client.waitForContainerSlot(0, marker.itemId(), marker.count(), INVENTORY_TIMEOUT);
            moved = slotMatched
                && client.moveContainerSlotToInventory(0, marker.itemId(), marker.count(), INVENTORY_TIMEOUT);
            empty = moved && client.waitForContainerSlotEmpty(0, INVENTORY_TIMEOUT);
            closed = opened && client.closeCurrentScreen(INVENTORY_TIMEOUT);
        }
        boolean passed = visible && opened && slotMatched && moved && empty && closed;
        observations.add(
            "two-client shared chest withdraw: " + (passed ? "passed" : "failed")
                + " target=" + coordinates(marker.chestTarget())
                + " item=" + marker.itemId()
                + " count=" + marker.count()
                + " approached=" + approached
                + " visible=" + visible
                + " open_use_result=" + openUse.result()
                + " screen_matched=" + opened
                + " slot_matched=" + slotMatched
                + " moved=" + moved
                + " empty=" + empty
                + " closed=" + closed
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientEarnedSharedChestObserveEmpty(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = sharedChestMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing shared chest marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        ChestStorageMarker marker = readSharedChestMarker(markerPath);
        boolean approached = client.approachBlock(marker.chestTarget(), APPROACH_TIMEOUT);
        observations.add(
            "two-client shared chest empty approach: " + (approached ? "passed" : "failed")
                + " target=" + coordinates(marker.chestTarget())
        );
        boolean visible = approached
            && client.waitForBlock(marker.chestTarget(), "minecraft:chest", BLOCK_TIMEOUT);
        ScenarioUseResult openUse = new ScenarioUseResult("skipped");
        boolean opened = false;
        boolean empty = false;
        boolean closed = false;
        if (visible) {
            openUse = client.useItemOn(marker.chestTarget(), client.selectedItem());
            opened = client.waitForScreenClassName(CONTAINER_SCREEN, INVENTORY_TIMEOUT);
            empty = opened && client.waitForContainerSlotEmpty(0, INVENTORY_TIMEOUT);
            closed = opened && client.closeCurrentScreen(INVENTORY_TIMEOUT);
        }
        boolean passed = visible && opened && empty && closed;
        observations.add(
            "two-client shared chest empty observe: " + (passed ? "passed" : "failed")
                + " target=" + coordinates(marker.chestTarget())
                + " item=" + marker.itemId()
                + " approached=" + approached
                + " visible=" + visible
                + " open_use_result=" + openUse.result()
                + " screen_matched=" + opened
                + " empty=" + empty
                + " closed=" + closed
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientEarnedTorchPlace(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        TorchPlacementResult placed = prepareTorchCraftPlace(id, observations, client);
        if (!"passed".equals(placed.report().result())) {
            return placed.report();
        }
        SharedBlockEditMarker marker = new SharedBlockEditMarker(
            placed.torchTarget(),
            new ScenarioBlockTarget(
                placed.approachTarget().x(),
                placed.approachTarget().y(),
                placed.approachTarget().z(),
                placed.approachTarget().face(),
                "playable-two-client-block-approach",
                placed.approachTarget().blockId()
            ),
            "minecraft:torch",
            "minecraft:torch"
        );
        writeSharedBlockEditMarker(sharedBlockEditMarkerPath(screenshotsDir), marker);
        observations.add(
            "two-client shared torch placement: passed target=" + coordinates(marker.target())
                + " block=" + marker.blockId()
                + " item=" + marker.itemId()
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runTwoClientEarnedTorchObserve(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = sharedBlockEditMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing shared block edit marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        SharedBlockEditMarker marker = readSharedBlockEditMarker(markerPath);
        boolean approached = client.approachBlock(marker.approachTarget(), APPROACH_TIMEOUT);
        boolean visible = approached && client.waitForBlock(marker.target(), marker.blockId(), BLOCK_TIMEOUT);
        observations.add(
            "two-client shared torch visibility: " + (visible ? "passed" : "failed")
                + " target=" + coordinates(marker.target())
                + " block=" + marker.blockId()
                + " approached=" + approached
                + " visible=" + visible
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(visible ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientEarnedTorchBreak(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = sharedBlockEditMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing shared block edit marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        SharedBlockEditMarker marker = readSharedBlockEditMarker(markerPath);
        boolean approached = client.approachBlock(marker.approachTarget(), APPROACH_TIMEOUT);
        boolean visible = approached && client.waitForBlock(marker.target(), marker.blockId(), BLOCK_TIMEOUT);
        ScenarioBreakResult broke = new ScenarioBreakResult(
            false,
            false,
            false,
            false,
            new ScenarioHeldItem("minecraft:air", 0)
        );
        boolean becameAir = false;
        ScenarioBreakResult pickup = new ScenarioBreakResult(
            false,
            false,
            false,
            false,
            new ScenarioHeldItem("minecraft:air", 0)
        );
        if (visible) {
            broke = client.breakBlockUntilDropVisible(marker.target(), marker.itemId(), BREAK_TIMEOUT);
            becameAir = broke.becameAir() && client.waitForBlock(marker.target(), "minecraft:air", BLOCK_TIMEOUT);
            if (broke.sawDrop()) {
                pickup = client.collectVisibleItemDrop(marker.target(), marker.itemId(), 1, PICKUP_TIMEOUT);
            }
        }
        boolean collectedByBreaker = pickup.pickupRestored();
        boolean passed = visible && broke.started() && broke.becameAir() && broke.sawDrop() && becameAir;
        observations.add(
            "two-client shared torch break: " + (passed ? "passed" : "failed")
                + " target=" + coordinates(marker.target())
                + " block=" + marker.blockId()
                + " item=" + marker.itemId()
                + " approached=" + approached
                + " visible=" + visible
                + " break_started=" + broke.started()
                + " became_air=" + becameAir
                + " saw_drop=" + broke.sawDrop()
                + " collected_by_breaker=" + collectedByBreaker
                + " held=" + pickup.selectedItem().itemId() + " x" + pickup.selectedItem().count()
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientEarnedTorchGoneObserve(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = sharedBlockEditMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing shared block edit marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        SharedBlockEditMarker marker = readSharedBlockEditMarker(markerPath);
        boolean approached = client.approachBlock(marker.approachTarget(), APPROACH_TIMEOUT);
        boolean removed = approached && client.waitForBlock(marker.target(), "minecraft:air", BLOCK_TIMEOUT);
        observations.add(
            "two-client shared torch removal visibility: " + (removed ? "passed" : "failed")
                + " target=" + coordinates(marker.target())
                + " block=" + marker.blockId()
                + " approached=" + approached
                + " removed=" + removed
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(removed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientPlayerObserve(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        ScenarioPlayerObservation primary = client.waitForVisiblePlayer(
            PRIMARY_CLIENT_USERNAME,
            ENTITY_SCAN_TIMEOUT
        );
        if (primary == null) {
            observations.add("failed: primary player not visible to secondary client: " + PRIMARY_CLIENT_USERNAME);
            return new ClientScenarioReport("failed", id, observations);
        }
        writePlayerVisibilityMarker(playerVisibilityMarkerPath(screenshotsDir), new PlayerVisibilityMarker(primary));
        observations.add(
            "two-client player visibility: passed player=" + primary.playerName()
                + " entity_id=" + primary.entityId()
                + " position=" + coordinates(primary)
                + " distance_squared=" + primary.distanceSquared()
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runTwoClientPlayerMovedObserve(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = playerVisibilityMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing player visibility marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        PlayerVisibilityMarker marker = readPlayerVisibilityMarker(markerPath);
        ScenarioPlayerObservation moved = client.waitForMovedPlayer(
            marker.observation().playerName(),
            marker.observation(),
            PLAYER_MOVEMENT_MIN_HORIZONTAL_DELTA,
            ENTITY_SCAN_TIMEOUT
        );
        boolean passed = moved != null;
        double horizontalDelta = passed ? horizontalDistance(marker.observation(), moved) : 0.0;
        observations.add(
            "two-client player movement visibility: " + (passed ? "passed" : "failed")
                + " player=" + marker.observation().playerName()
                + " before=" + coordinates(marker.observation())
                + " after=" + (moved == null ? "missing" : coordinates(moved))
                + " min_horizontal_delta=" + PLAYER_MOVEMENT_MIN_HORIZONTAL_DELTA
                + " horizontal_delta=" + horizontalDelta
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientChatSend(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        client.sendChatMessage(TWO_CLIENT_CHAT_MESSAGE_TEXT);
        observations.add("two-client chat send: passed message=" + TWO_CLIENT_CHAT_MESSAGE_TEXT);
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runTwoClientChatObserve(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        String expectedText = "<" + PRIMARY_CLIENT_USERNAME + "> " + TWO_CLIENT_CHAT_MESSAGE_TEXT;
        boolean seen = client.waitForChatMessage(expectedText, CHAT_TIMEOUT);
        observations.add(
            "two-client chat observe: " + (seen ? "passed" : "failed")
                + " expected=" + expectedText
        );
        return new ClientScenarioReport(seen ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientPlayerDisconnectVisible(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        ScenarioPlayerObservation primary = client.waitForVisiblePlayer(
            PRIMARY_CLIENT_USERNAME,
            ENTITY_SCAN_TIMEOUT
        );
        if (primary == null) {
            observations.add("failed: primary player not visible before disconnect: " + PRIMARY_CLIENT_USERNAME);
            return new ClientScenarioReport("failed", id, observations);
        }
        observations.add(
            "two-client player pre-disconnect visibility: passed player=" + primary.playerName()
                + " entity_id=" + primary.entityId()
                + " position=" + coordinates(primary)
                + " distance_squared=" + primary.distanceSquared()
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runTwoClientPlayerGoneObserve(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        boolean removed = client.waitForNoVisiblePlayer(PRIMARY_CLIENT_USERNAME, ENTITY_SCAN_TIMEOUT);
        observations.add(
            "two-client player disconnect removal: " + (removed ? "passed" : "failed")
                + " player=" + PRIMARY_CLIENT_USERNAME
        );
        return new ClientScenarioReport(removed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientPlayerReconnectVisible(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        ScenarioPlayerObservation primary = client.waitForVisiblePlayer(
            PRIMARY_CLIENT_USERNAME,
            ENTITY_SCAN_TIMEOUT
        );
        if (primary == null) {
            observations.add("failed: primary player not visible before reconnect: " + PRIMARY_CLIENT_USERNAME);
            return new ClientScenarioReport("failed", id, observations);
        }
        writePlayerVisibilityMarker(playerVisibilityMarkerPath(screenshotsDir), new PlayerVisibilityMarker(primary));
        observations.add(
            "two-client player pre-reconnect visibility: passed player=" + primary.playerName()
                + " entity_id=" + primary.entityId()
                + " position=" + coordinates(primary)
                + " distance_squared=" + primary.distanceSquared()
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runTwoClientPlayerReconnectGoneObserve(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        boolean removed = client.waitForNoVisiblePlayer(PRIMARY_CLIENT_USERNAME, ENTITY_SCAN_TIMEOUT);
        observations.add(
            "two-client player reconnect removal: " + (removed ? "passed" : "failed")
                + " player=" + PRIMARY_CLIENT_USERNAME
        );
        return new ClientScenarioReport(removed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientPlayerReconnectedObserve(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = playerVisibilityMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing player visibility marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        PlayerVisibilityMarker marker = readPlayerVisibilityMarker(markerPath);
        ScenarioPlayerObservation reconnected = client.waitForVisiblePlayer(
            marker.observation().playerName(),
            ENTITY_SCAN_TIMEOUT
        );
        boolean visible = reconnected != null;
        boolean replaced = visible && reconnected.entityId() != marker.observation().entityId();
        observations.add(
            "two-client player reconnect visibility: " + (replaced ? "passed" : "failed")
                + " player=" + marker.observation().playerName()
                + " old_entity_id=" + marker.observation().entityId()
                + " new_entity_id=" + (reconnected == null ? "missing" : reconnected.entityId())
                + " old_position=" + coordinates(marker.observation())
                + " new_position=" + (reconnected == null ? "missing" : coordinates(reconnected))
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(replaced ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientPlayerDeathBaseline(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        ScenarioPlayerObservation primary = client.waitForVisiblePlayer(
            PRIMARY_CLIENT_USERNAME,
            ENTITY_SCAN_TIMEOUT
        );
        if (primary == null) {
            observations.add("failed: primary player not visible before death: " + PRIMARY_CLIENT_USERNAME);
            return new ClientScenarioReport("failed", id, observations);
        }
        writePlayerVisibilityMarker(playerVisibilityMarkerPath(screenshotsDir), new PlayerVisibilityMarker(primary));
        observations.add(
            "two-client player pre-death visibility: passed player=" + primary.playerName()
                + " entity_id=" + primary.entityId()
                + " position=" + coordinates(primary)
                + " distance_squared=" + primary.distanceSquared()
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runTwoClientCampfireDeathRespawn(
        String id,
        List<String> observations,
        ScenarioClient client
    ) throws Exception {
        CampfireDeathRespawnResult death = performEarnedCampfireDeathRespawn(id, observations, client, false);
        if (!"passed".equals(death.report().result())) {
            return death.report();
        }
        observations.add("two-client campfire death/respawn: passed natural_hazard=campfire respawn=true");
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runTwoClientPlayerPostRespawnMovedObserve(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = playerVisibilityMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing player visibility marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }

        PlayerVisibilityMarker marker = readPlayerVisibilityMarker(markerPath);
        ScenarioPlayerObservation moved = client.waitForMovedPlayer(
            marker.observation().playerName(),
            marker.observation(),
            PLAYER_MOVEMENT_MIN_HORIZONTAL_DELTA,
            ENTITY_SCAN_TIMEOUT
        );
        boolean passed = moved != null;
        double horizontalDelta = passed ? horizontalDistance(marker.observation(), moved) : 0.0;
        observations.add(
            "two-client player post-respawn movement visibility: " + (passed ? "passed" : "failed")
                + " player=" + marker.observation().playerName()
                + " before_death=" + coordinates(marker.observation())
                + " after_respawn_move=" + (moved == null ? "missing" : coordinates(moved))
                + " min_horizontal_delta=" + PLAYER_MOVEMENT_MIN_HORIZONTAL_DELTA
                + " horizontal_delta=" + horizontalDelta
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(passed ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientInventoryDropPrimary(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        ScenarioBlockTarget log = findReachableNaturalLog(id, observations, client, "inventory drop log");
        if (log == null) {
            return new ClientScenarioReport("blocked", id, observations);
        }

        ScenarioBreakResult broke = client.breakBlockUntilDropVisible(log, log.blockId(), BREAK_TIMEOUT);
        ScenarioBreakResult pickup = client.collectVisibleItemDrop(log, log.blockId(), 1, PICKUP_TIMEOUT);
        boolean collected = broke.started()
            && broke.becameAir()
            && broke.sawDrop()
            && pickup.pickupRestored();
        observations.add(
            "inventory drop source log pickup: " + (collected ? "passed" : "failed")
                + " target=" + coordinates(log)
                + " item=" + log.blockId()
                + " break_started=" + broke.started()
                + " became_air=" + broke.becameAir()
                + " saw_drop=" + broke.sawDrop()
                + " pickup_restored=" + pickup.pickupRestored()
        );
        if (!collected) {
            return new ClientScenarioReport("failed", id, observations);
        }

        ScenarioHeldItem held = client.selectHotbarItem(log.blockId(), 1, HOTBAR_TIMEOUT);
        if (!held.matches(log.blockId(), 1)) {
            observations.add("blocked: earned inventory drop item is not selectable item=" + log.blockId());
            return new ClientScenarioReport("blocked", id, observations);
        }
        ScenarioBlockTarget dropTarget = client.dropSelectedItem(log.blockId(), 1, PICKUP_TIMEOUT);
        boolean visible = client.waitForVisibleItemDrop(log.blockId(), dropTarget, PICKUP_TIMEOUT);
        observations.add(
            "two-client inventory drop: " + (visible ? "passed" : "failed")
                + " target=" + coordinates(dropTarget)
                + " item=" + log.blockId()
                + " selected=" + held.itemId() + " x" + held.count()
                + " visible=" + visible
        );
        if (!visible) {
            return new ClientScenarioReport("failed", id, observations);
        }
        writeSharedLogDropMarker(inventoryDropMarkerPath(screenshotsDir), dropTarget, log.blockId());
        observations.add("inventory drop marker: passed target=" + coordinates(dropTarget) + " item=" + log.blockId());
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport("passed", id, observations);
    }

    private ClientScenarioReport runTwoClientInventoryDropObserve(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = inventoryDropMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing inventory drop marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }
        SharedLogDropMarker marker = readSharedLogDropMarker(markerPath, "playable-two-client-inventory-drop-marker");
        boolean visible = client.waitForVisibleItemDrop(marker.itemId(), marker.target(), PICKUP_TIMEOUT);
        observations.add(
            "two-client inventory drop visibility: " + (visible ? "passed" : "failed")
                + " target=" + coordinates(marker.target())
                + " item=" + marker.itemId()
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(visible ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientInventoryDropSecondaryPickup(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = inventoryDropMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing inventory drop marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }
        SharedLogDropMarker marker = readSharedLogDropMarker(markerPath, "playable-two-client-inventory-drop-marker");
        ScenarioBreakResult pickup = client.collectVisibleItemDrop(
            marker.target(),
            marker.itemId(),
            1,
            PICKUP_TIMEOUT
        );
        boolean collected = pickup.becameAir() && pickup.pickupRestored();
        observations.add(
            "two-client inventory drop secondary pickup: " + (collected ? "passed" : "failed")
                + " target=" + coordinates(marker.target())
                + " item=" + marker.itemId()
                + " saw_drop=" + pickup.sawDrop()
                + " drop_gone=" + pickup.becameAir()
                + " pickup_restored=" + pickup.pickupRestored()
                + " held=" + pickup.selectedItem().itemId() + " x" + pickup.selectedItem().count()
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(collected ? "passed" : "failed", id, observations);
    }

    private ClientScenarioReport runTwoClientInventoryDropGoneObserve(
        String id,
        List<String> observations,
        Path screenshotsDir,
        ScenarioClient client
    ) throws Exception {
        Path markerPath = inventoryDropMarkerPath(screenshotsDir);
        if (!Files.isRegularFile(markerPath)) {
            observations.add("missing inventory drop marker file: " + markerPath);
            return new ClientScenarioReport("failed", id, observations);
        }
        SharedLogDropMarker marker = readSharedLogDropMarker(markerPath, "playable-two-client-inventory-drop-marker");
        boolean removed = client.waitForNoVisibleItemDrop(marker.itemId(), marker.target(), PICKUP_TIMEOUT);
        observations.add(
            "two-client inventory drop removal: " + (removed ? "passed" : "failed")
                + " target=" + coordinates(marker.target())
                + " item=" + marker.itemId()
        );
        observations.add("screenshots directory available to driver: " + screenshotsDir);
        return new ClientScenarioReport(removed ? "passed" : "failed", id, observations);
    }

    private static String coordinates(ScenarioBlockTarget target) {
        return target.x() + "," + target.y() + "," + target.z() + "/" + target.face();
    }

    private static String coordinates(ScenarioPlayerObservation player) {
        return player.x() + "," + player.y() + "," + player.z();
    }

    private static double horizontalDistance(ScenarioPlayerObservation a, ScenarioPlayerObservation b) {
        double dx = a.x() - b.x();
        double dz = a.z() - b.z();
        return Math.sqrt(dx * dx + dz * dz);
    }

    private static boolean usesReachOnlyLogClose(String id) {
        return EARNED_CHEST_STORAGE_ID.equals(id) || CHEST_STORAGE_SAVE_RESTART_BEFORE_ID.equals(id);
    }

    private static Path saveRestartMarkerPath(Path screenshotsDir) {
        Path runDir = screenshotsDir.getParent();
        return (runDir == null ? Path.of(".") : runDir).resolve(SAVE_RESTART_MARKER_FILE);
    }

    private static Path chestStorageMarkerPath(Path screenshotsDir) {
        Path runDir = screenshotsDir.getParent();
        return (runDir == null ? Path.of(".") : runDir).resolve(CHEST_STORAGE_MARKER_FILE);
    }

    private static Path generatedRuinCacheMarkerPath(Path screenshotsDir) {
        Path runDir = screenshotsDir.getParent();
        return (runDir == null ? Path.of(".") : runDir).resolve(GENERATED_RUIN_CACHE_MARKER_FILE);
    }

    private static Path sharedLogDropMarkerPath(Path screenshotsDir) {
        Path runDir = screenshotsDir.getParent();
        return (runDir == null ? Path.of(".") : runDir).resolve(SHARED_LOG_DROP_MARKER_FILE);
    }

    private static Path inventoryDropMarkerPath(Path screenshotsDir) {
        Path runDir = screenshotsDir.getParent();
        return (runDir == null ? Path.of(".") : runDir).resolve(INVENTORY_DROP_MARKER_FILE);
    }

    private static Path sharedChestMarkerPath(Path screenshotsDir) {
        Path runDir = screenshotsDir.getParent();
        return (runDir == null ? Path.of(".") : runDir).resolve(SHARED_CHEST_MARKER_FILE);
    }

    private static Path sharedBlockEditMarkerPath(Path screenshotsDir) {
        Path runDir = screenshotsDir.getParent();
        return (runDir == null ? Path.of(".") : runDir).resolve(SHARED_BLOCK_EDIT_MARKER_FILE);
    }

    private static Path playerVisibilityMarkerPath(Path screenshotsDir) {
        Path runDir = screenshotsDir.getParent();
        return (runDir == null ? Path.of(".") : runDir).resolve(PLAYER_VISIBILITY_MARKER_FILE);
    }

    private static void writeMarker(Path path, ScenarioBlockTarget target) throws IOException {
        Files.createDirectories(path.getParent());
        Files.writeString(
            path,
            "x=" + target.x() + "\n"
                + "y=" + target.y() + "\n"
                + "z=" + target.z() + "\n"
                + "face=" + target.face() + "\n"
        );
    }

    private static void writeSharedLogDropMarker(
        Path path,
        ScenarioBlockTarget target,
        String itemId
    ) throws IOException {
        Files.createDirectories(path.getParent());
        Files.writeString(
            path,
            "x=" + target.x() + "\n"
                + "y=" + target.y() + "\n"
                + "z=" + target.z() + "\n"
                + "face=" + target.face() + "\n"
                + "item=" + itemId + "\n"
        );
    }

    private static void writeSharedBlockEditMarker(Path path, SharedBlockEditMarker marker) throws IOException {
        Files.createDirectories(path.getParent());
        Files.writeString(
            path,
            "x=" + marker.target().x() + "\n"
                + "y=" + marker.target().y() + "\n"
                + "z=" + marker.target().z() + "\n"
                + "face=" + marker.target().face() + "\n"
                + "approach_x=" + marker.approachTarget().x() + "\n"
                + "approach_y=" + marker.approachTarget().y() + "\n"
                + "approach_z=" + marker.approachTarget().z() + "\n"
                + "approach_face=" + marker.approachTarget().face() + "\n"
                + "approach_block=" + marker.approachTarget().blockId() + "\n"
                + "block=" + marker.blockId() + "\n"
                + "item=" + marker.itemId() + "\n"
        );
    }

    private static void writePlayerVisibilityMarker(
        Path path,
        PlayerVisibilityMarker marker
    ) throws IOException {
        Files.createDirectories(path.getParent());
        ScenarioPlayerObservation observation = marker.observation();
        Files.writeString(
            path,
            "player=" + observation.playerName() + "\n"
                + "entity_id=" + observation.entityId() + "\n"
                + "x=" + observation.x() + "\n"
                + "y=" + observation.y() + "\n"
                + "z=" + observation.z() + "\n"
                + "distance_squared=" + observation.distanceSquared() + "\n"
        );
    }

    private static ScenarioBlockTarget reachableUseTarget(
        ScenarioClient client,
        ScenarioBlockTarget original
    ) throws Exception {
        ScenarioBlockTarget reachable = client.findBreakableBlock(
            List.of(original.blockId()),
            ScenarioReach.WITHIN_SURVIVAL_REACH
        );
        if (reachable == null
            || reachable.x() != original.x()
            || reachable.y() != original.y()
            || reachable.z() != original.z()) {
            return original;
        }
        return new ScenarioBlockTarget(
            original.x(),
            original.y(),
            original.z(),
            reachable.face(),
            original.label(),
            original.blockId()
        );
    }

    private static void writeChestStorageMarker(Path path, ChestStorageResult stored) throws IOException {
        Files.createDirectories(path.getParent());
        Files.writeString(
            path,
            "x=" + stored.chestTarget().x() + "\n"
                + "y=" + stored.chestTarget().y() + "\n"
                + "z=" + stored.chestTarget().z() + "\n"
                + "face=" + stored.chestTarget().face() + "\n"
                + "item=" + stored.itemId() + "\n"
                + "count=" + stored.count() + "\n"
        );
    }

    private static void writeGeneratedRuinCacheMarker(Path path, GeneratedRuinCacheMarker marker)
        throws IOException {
        Files.createDirectories(path.getParent());
        StringBuilder content = new StringBuilder()
            .append("x=").append(marker.chestTarget().x()).append('\n')
            .append("y=").append(marker.chestTarget().y()).append('\n')
            .append("z=").append(marker.chestTarget().z()).append('\n')
            .append("face=").append(marker.chestTarget().face()).append('\n');
        for (GeneratedRuinLoot loot : marker.loot()) {
            String key = loot.itemId().substring("minecraft:".length());
            content.append(key).append("_count=").append(loot.count()).append('\n');
            content.append(key).append("_slot=").append(loot.slot()).append('\n');
        }
        Files.writeString(path, content.toString());
    }

    private static GeneratedRuinCacheMarker readGeneratedRuinCacheMarker(Path path) throws IOException {
        Integer x = null;
        Integer y = null;
        Integer z = null;
        String face = null;
        List<GeneratedRuinLoot> loot = new ArrayList<>();
        Map<String, String> values = new java.util.HashMap<>();
        for (String line : Files.readAllLines(path)) {
            String[] parts = line.split("=", 2);
            if (parts.length == 2) {
                values.put(parts[0], parts[1]);
            }
        }
        try {
            x = Integer.valueOf(values.get("x"));
            y = Integer.valueOf(values.get("y"));
            z = Integer.valueOf(values.get("z"));
            face = values.get("face");
            for (GeneratedRuinLoot expected : GENERATED_RUIN_LOOT) {
                String key = expected.itemId().substring("minecraft:".length());
                int count = Integer.parseInt(values.get(key + "_count"));
                int slot = Integer.parseInt(values.get(key + "_slot"));
                if (
                    count != expected.count()
                        || slot < 0
                        || slot >= GENERATED_RUIN_CHEST_SLOT_COUNT
                ) {
                    throw new IOException("invalid generated ruin loot marker entry for " + expected.itemId());
                }
                loot.add(new GeneratedRuinLoot(expected.itemId(), count, slot));
            }
        } catch (NullPointerException | NumberFormatException error) {
            throw new IOException("invalid generated ruin cache marker file: " + path, error);
        }
        if (
            x != GENERATED_RUIN_CENTER_X
                || z != GENERATED_RUIN_CENTER_Z
                || !GENERATED_RUIN_SUPPORTED_FACES.contains(face)
                || loot.stream().map(GeneratedRuinLoot::slot).distinct().count() != loot.size()
        ) {
            throw new IOException("invalid generated ruin cache marker file: " + path);
        }
        return new GeneratedRuinCacheMarker(
            new ScenarioBlockTarget(x, y, z, face, "generated-ruin-cache", "minecraft:chest"),
            List.copyOf(loot)
        );
    }

    private static ScenarioBlockTarget readMarker(Path path, String label) throws IOException {
        return readMarker(path, label, "minecraft:crafting_table");
    }

    private static ScenarioBlockTarget readMarker(Path path, String label, String blockId) throws IOException {
        Integer x = null;
        Integer y = null;
        Integer z = null;
        String face = null;
        for (String line : Files.readAllLines(path)) {
            String[] parts = line.split("=", 2);
            if (parts.length != 2) {
                continue;
            }
            switch (parts[0]) {
                case "x" -> x = Integer.parseInt(parts[1]);
                case "y" -> y = Integer.parseInt(parts[1]);
                case "z" -> z = Integer.parseInt(parts[1]);
                case "face" -> face = parts[1];
                default -> {
                }
            }
        }
        if (x == null || y == null || z == null || face == null) {
            throw new IOException("invalid restart marker file: " + path);
        }
        return new ScenarioBlockTarget(x, y, z, face, label, blockId);
    }

    private static SharedLogDropMarker readSharedLogDropMarker(Path path) throws IOException {
        return readSharedLogDropMarker(path, "playable-two-client-drop-marker");
    }

    private static SharedLogDropMarker readSharedLogDropMarker(Path path, String label) throws IOException {
        Integer x = null;
        Integer y = null;
        Integer z = null;
        String face = null;
        String item = null;
        for (String line : Files.readAllLines(path)) {
            String[] parts = line.split("=", 2);
            if (parts.length != 2) {
                continue;
            }
            switch (parts[0]) {
                case "x" -> x = Integer.parseInt(parts[1]);
                case "y" -> y = Integer.parseInt(parts[1]);
                case "z" -> z = Integer.parseInt(parts[1]);
                case "face" -> face = parts[1];
                case "item" -> item = parts[1];
                default -> {
                }
            }
        }
        if (x == null || y == null || z == null || face == null || item == null) {
            throw new IOException("invalid shared log drop marker file: " + path);
        }
        return new SharedLogDropMarker(
            new ScenarioBlockTarget(x, y, z, face, label, item),
            item
        );
    }

    private static SharedBlockEditMarker readSharedBlockEditMarker(Path path) throws IOException {
        Integer x = null;
        Integer y = null;
        Integer z = null;
        String face = null;
        Integer approachX = null;
        Integer approachY = null;
        Integer approachZ = null;
        String approachFace = null;
        String approachBlock = null;
        String block = null;
        String item = null;
        for (String line : Files.readAllLines(path)) {
            String[] parts = line.split("=", 2);
            if (parts.length != 2) {
                continue;
            }
            switch (parts[0]) {
                case "x" -> x = Integer.parseInt(parts[1]);
                case "y" -> y = Integer.parseInt(parts[1]);
                case "z" -> z = Integer.parseInt(parts[1]);
                case "face" -> face = parts[1];
                case "approach_x" -> approachX = Integer.parseInt(parts[1]);
                case "approach_y" -> approachY = Integer.parseInt(parts[1]);
                case "approach_z" -> approachZ = Integer.parseInt(parts[1]);
                case "approach_face" -> approachFace = parts[1];
                case "approach_block" -> approachBlock = parts[1];
                case "block" -> block = parts[1];
                case "item" -> item = parts[1];
                default -> {
                }
            }
        }
        if (
            x == null
                || y == null
                || z == null
                || face == null
                || approachX == null
                || approachY == null
                || approachZ == null
                || approachFace == null
                || approachBlock == null
                || block == null
                || item == null
                || face.isBlank()
                || approachFace.isBlank()
                || approachBlock.isBlank()
                || block.isBlank()
                || item.isBlank()
        ) {
            throw new IOException("invalid shared block edit marker file: " + path);
        }
        ScenarioBlockTarget target = new ScenarioBlockTarget(
            x,
            y,
            z,
            face,
            "playable-two-client-block-marker",
            block
        );
        ScenarioBlockTarget approachTarget = new ScenarioBlockTarget(
            approachX,
            approachY,
            approachZ,
            approachFace,
            "playable-two-client-block-approach",
            approachBlock
        );
        return new SharedBlockEditMarker(target, approachTarget, block, item);
    }

    private static PlayerVisibilityMarker readPlayerVisibilityMarker(Path path) throws IOException {
        String player = null;
        Integer entityId = null;
        Double x = null;
        Double y = null;
        Double z = null;
        Double distanceSquared = null;
        for (String line : Files.readAllLines(path)) {
            String[] parts = line.split("=", 2);
            if (parts.length != 2) {
                continue;
            }
            switch (parts[0]) {
                case "player" -> player = parts[1];
                case "entity_id" -> entityId = Integer.parseInt(parts[1]);
                case "x" -> x = Double.parseDouble(parts[1]);
                case "y" -> y = Double.parseDouble(parts[1]);
                case "z" -> z = Double.parseDouble(parts[1]);
                case "distance_squared" -> distanceSquared = Double.parseDouble(parts[1]);
                default -> {
                }
            }
        }
        if (
            player == null
                || player.isBlank()
                || entityId == null
                || x == null
                || y == null
                || z == null
                || distanceSquared == null
        ) {
            throw new IOException("invalid player visibility marker file: " + path);
        }
        return new PlayerVisibilityMarker(new ScenarioPlayerObservation(player, entityId, x, y, z, distanceSquared));
    }

    private static ChestStorageMarker readChestStorageMarker(Path path) throws IOException {
        return readChestStorageMarker(path, "chest-marker");
    }

    private static ChestStorageMarker readSharedChestMarker(Path path) throws IOException {
        return readChestStorageMarker(path, "playable-two-client-chest-marker");
    }

    private static ChestStorageMarker readChestStorageMarker(Path path, String label) throws IOException {
        String item = null;
        Integer count = null;
        for (String line : Files.readAllLines(path)) {
            String[] parts = line.split("=", 2);
            if (parts.length != 2) {
                continue;
            }
            switch (parts[0]) {
                case "item" -> item = parts[1];
                case "count" -> count = Integer.parseInt(parts[1]);
                default -> {
                }
            }
        }
        if (item == null || count == null || count <= 0) {
            throw new IOException("invalid chest storage marker file: " + path);
        }
        return new ChestStorageMarker(readMarker(path, label, "minecraft:chest"), item, count);
    }

    private record CraftingTableOpenResult(ClientScenarioReport report, ScenarioBlockTarget tableTarget) {
    }
}
