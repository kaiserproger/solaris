package dev.solaris.agent.javaagent;

final class SheepWoolColor {
    private SheepWoolColor() {
    }

    static String itemId(String dyeColorName) {
        return switch (dyeColorName) {
            case "WHITE" -> "minecraft:white_wool";
            case "ORANGE" -> "minecraft:orange_wool";
            case "MAGENTA" -> "minecraft:magenta_wool";
            case "LIGHT_BLUE" -> "minecraft:light_blue_wool";
            case "YELLOW" -> "minecraft:yellow_wool";
            case "LIME" -> "minecraft:lime_wool";
            case "PINK" -> "minecraft:pink_wool";
            case "GRAY" -> "minecraft:gray_wool";
            case "LIGHT_GRAY" -> "minecraft:light_gray_wool";
            case "CYAN" -> "minecraft:cyan_wool";
            case "PURPLE" -> "minecraft:purple_wool";
            case "BLUE" -> "minecraft:blue_wool";
            case "BROWN" -> "minecraft:brown_wool";
            case "GREEN" -> "minecraft:green_wool";
            case "RED" -> "minecraft:red_wool";
            case "BLACK" -> "minecraft:black_wool";
            default -> throw new IllegalArgumentException("unsupported sheep dye color " + dyeColorName);
        };
    }
}
