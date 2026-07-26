package dev.solaris.loader.fabric;

import dev.solaris.loader.LoaderInteractionDefinition;
import dev.solaris.loader.LoaderScreenDefinition;
import java.util.List;
import java.util.function.Consumer;
import net.minecraft.client.gui.components.Button;
import net.minecraft.client.gui.components.ItemDisplayWidget;
import net.minecraft.client.gui.components.MultiLineTextWidget;
import net.minecraft.client.gui.components.StringWidget;
import net.minecraft.client.gui.layouts.FrameLayout;
import net.minecraft.client.gui.layouts.LinearLayout;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.network.chat.Component;
import net.minecraft.world.item.ItemStack;

final class LoaderTextScreen extends Screen {
    private final Component body;
    private final List<LoaderInteractionDefinition> interactions;
    private final Consumer<LoaderInteractionDefinition> action;
    private final List<ItemStack> displayItems;
    private final LinearLayout layout = LinearLayout.vertical().spacing(8);

    LoaderTextScreen(
            LoaderScreenDefinition definition,
            List<LoaderInteractionDefinition> interactions,
            Consumer<LoaderInteractionDefinition> action,
            List<ItemStack> displayItems) {
        super(Component.literal(definition.title()));
        body = Component.literal(definition.body());
        this.interactions = List.copyOf(interactions);
        this.action = action;
        this.displayItems = List.copyOf(displayItems);
    }

    @Override
    protected void init() {
        layout.defaultCellSetting().alignHorizontallyCenter();
        layout.addChild(new StringWidget(title, font));
        layout.addChild(new MultiLineTextWidget(body, font)
                .setMaxWidth(width - 50)
                .setMaxRows(15)
                .setCentered(true));
        for (ItemStack stack : displayItems) {
            layout.addChild(new ItemDisplayWidget(
                    minecraft,
                    8,
                    8,
                    32,
                    32,
                    stack.getHoverName(),
                    stack,
                    true,
                    true));
        }
        for (LoaderInteractionDefinition interaction : interactions) {
            layout.addChild(Button.builder(
                            Component.literal(interaction.label()),
                            ignored -> action.accept(interaction))
                    .width(200)
                    .build());
        }
        layout.visitWidgets(this::addRenderableWidget);
        repositionElements();
    }

    @Override
    protected void repositionElements() {
        layout.arrangeElements();
        FrameLayout.centerInRectangle(layout, getRectangle());
    }
}
