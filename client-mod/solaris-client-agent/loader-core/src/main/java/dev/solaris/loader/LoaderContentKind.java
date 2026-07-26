package dev.solaris.loader;

import com.google.gson.annotations.SerializedName;

public enum LoaderContentKind {
    @SerializedName("blocks")
    BLOCKS,
    @SerializedName("items")
    ITEMS,
    @SerializedName("screens")
    SCREENS,
    @SerializedName("assets")
    ASSETS,
    @SerializedName("interactions")
    INTERACTIONS
}
