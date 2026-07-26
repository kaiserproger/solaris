package dev.solaris.loader;

import com.google.gson.annotations.SerializedName;

public enum LoaderPermission {
    @SerializedName("register_blocks")
    REGISTER_BLOCKS,
    @SerializedName("register_items")
    REGISTER_ITEMS,
    @SerializedName("open_screens")
    OPEN_SCREENS,
    @SerializedName("load_assets")
    LOAD_ASSETS,
    @SerializedName("send_interactions")
    SEND_INTERACTIONS;

    public String wireName() {
        return switch (this) {
            case REGISTER_BLOCKS -> "register_blocks";
            case REGISTER_ITEMS -> "register_items";
            case OPEN_SCREENS -> "open_screens";
            case LOAD_ASSETS -> "load_assets";
            case SEND_INTERACTIONS -> "send_interactions";
        };
    }

    public static LoaderPermission fromWireName(String value) {
        return switch (value) {
            case "register_blocks" -> REGISTER_BLOCKS;
            case "register_items" -> REGISTER_ITEMS;
            case "open_screens" -> OPEN_SCREENS;
            case "load_assets" -> LOAD_ASSETS;
            case "send_interactions" -> SEND_INTERACTIONS;
            default -> throw new IllegalArgumentException("unknown Solaris Loader permission " + value);
        };
    }
}
