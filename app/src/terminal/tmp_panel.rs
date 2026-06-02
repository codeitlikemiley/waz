use crate::appearance::Appearance;
use crate::terminal::input::{Input, MenuPositioning};
use warp_completer::signatures::tmp::{CommandEntry, TokenType};
use warpui::elements::{
    Border, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, DropShadow,
    Element, Empty, Flex, ParentElement, Radius, Text,
};
use warpui::fonts::{Properties, Weight};

impl Input {
    pub(super) fn render_tmp_form_panel(
        &self,
        appearance: &Appearance,
        _menu_positioning: MenuPositioning,
        command_entry: &CommandEntry,
        active_token_index: usize,
        token_values: &[String],
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let corner_radius = CornerRadius::with_all(Radius::Pixels(6.));
        
        let mut form_column = Flex::column();

        // 1. Header showing tool and command description
        form_column.add_child(
            Container::new(
                Flex::row()
                    .with_child(
                        Text::new_inline(
                            format!("⚙ TMP: {}", command_entry.group.to_uppercase()),
                            appearance.ui_font_family(),
                            appearance.ui_font_size(),
                        )
                        .with_style(Properties::default().weight(Weight::Bold))
                        .with_color(theme.main_text_color(theme.surface_2()).into_solid())
                        .finish()
                    )
                    .finish()
            )
            .with_padding_left(10.)
            .with_padding_right(10.)
            .with_padding_top(8.)
            .with_padding_bottom(4.)
            .finish()
        );

        form_column.add_child(
            Container::new(
                Text::new(
                    command_entry.description.clone(),
                    appearance.ui_font_family(),
                    appearance.ui_font_size() * 0.9,
                )
                .with_color(theme.sub_text_color(theme.surface_2()).into())
                .finish()
            )
            .with_padding_left(10.)
            .with_padding_right(10.)
            .with_padding_bottom(8.)
            .finish()
        );

        // Spacer line
        form_column.add_child(
            ConstrainedBox::new(
                Container::new(Empty::new().finish())
                    .with_background_color(theme.outline().into())
                    .finish()
            )
            .with_height(1.)
            .finish()
        );

        // 2. Render arguments / tokens list
        let mut tokens_list = Flex::column();
        for (i, token) in command_entry.tokens.iter().enumerate() {
            let is_active = i == active_token_index;
            let value = token_values.get(i).cloned().unwrap_or_default();
            
            let req_indicator = if token.required { " *" } else { "" };
            
            let mut label_row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center);

            // Active indicator pointer
            let cursor = if is_active { "▸ " } else { "  " };
            label_row.add_child(
                Text::new_inline(
                    cursor.to_string(),
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(theme.active_ui_text_color().into())
                .finish()
            );

            // Token name
            label_row.add_child(
                Text::new_inline(
                    format!("{}{}: ", token.name, req_indicator),
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_style(Properties::default().weight(if is_active { Weight::Bold } else { Weight::Normal }))
                .with_color(if is_active {
                    theme.active_ui_text_color().into()
                } else {
                    theme.sub_text_color(theme.surface_2()).into()
                })
                .finish()
            );

            // Token type/value display
            let value_display = match token.token_type {
                TokenType::Boolean => {
                    if value == "true" { "☑ Yes" } else { "☐ No" }
                }
                TokenType::Enum => {
                    if value.is_empty() {
                        "<select>"
                    } else {
                        value.as_str()
                    }
                }
                _ => {
                    if value.is_empty() {
                        "..."
                    } else {
                        value.as_str()
                    }
                }
            };

            label_row.add_child(
                Container::new(
                    Text::new_inline(
                        value_display.to_string(),
                        appearance.ui_font_family(),
                        appearance.ui_font_size(),
                    )
                    .with_color(if is_active {
                        theme.main_text_color(theme.surface_2()).into_solid()
                    } else {
                        theme.sub_text_color(theme.surface_2()).into()
                    })
                    .finish()
                )
                .with_padding_left(4.)
                .finish()
            );

            let mut token_container = Container::new(label_row.finish())
                .with_padding_top(4.)
                .with_padding_bottom(4.)
                .with_padding_left(8.)
                .with_padding_right(8.);

            if is_active {
                token_container = token_container
                    .with_background_color(theme.outline().into());
            }

            tokens_list.add_child(token_container.finish());

            // Render token description if it's active
            if is_active && !token.description.is_empty() {
                tokens_list.add_child(
                    Container::new(
                        Text::new(
                            format!("ℹ {}", token.description),
                            appearance.ui_font_family(),
                            appearance.ui_font_size() * 0.85,
                        )
                        .with_color(theme.nonactive_ui_text_color().into())
                        .finish()
                    )
                    .with_padding_left(24.)
                    .with_padding_top(2.)
                    .with_padding_bottom(4.)
                    .finish()
                );

                // Show valid enum values if active and enum
                if token.token_type == TokenType::Enum {
                    if let Some(ref vals) = token.values {
                        if !vals.is_empty() {
                            tokens_list.add_child(
                                Container::new(
                                    Text::new(
                                        format!("Allowed: {:?}", vals),
                                        appearance.ui_font_family(),
                                        appearance.ui_font_size() * 0.85,
                                    )
                                    .with_color(theme.nonactive_ui_text_color().into())
                                    .finish()
                                )
                                .with_padding_left(24.)
                                .with_padding_bottom(4.)
                                .finish()
                            );
                        }
                    }
                }
            }
        }

        form_column.add_child(
            Container::new(tokens_list.finish())
                .with_padding_top(6.)
                .with_padding_bottom(6.)
                .finish()
        );

        // Spacer line
        form_column.add_child(
            ConstrainedBox::new(
                Container::new(Empty::new().finish())
                    .with_background_color(theme.outline().into())
                    .finish()
            )
            .with_height(1.)
            .finish()
        );

        // 3. Command Preview
        let mut assembled = command_entry.command.clone();
        for (i, token) in command_entry.tokens.iter().enumerate() {
            let val = token_values.get(i).cloned().unwrap_or_default();
            let replacement = if val.is_empty() {
                format!("<{}>", token.name)
            } else {
                val
            };
            assembled = assembled.replace(&format!("<{}>", token.name), &replacement);
        }

        form_column.add_child(
            Container::new(
                Flex::row()
                    .with_child(
                        Text::new_inline(
                            "Preview: ".to_string(),
                            appearance.ui_font_family(),
                            appearance.ui_font_size() * 0.9,
                        )
                        .with_color(theme.sub_text_color(theme.surface_2()).into())
                        .finish()
                    )
                    .with_child(
                        Text::new_inline(
                            assembled,
                            appearance.monospace_font_family(),
                            appearance.monospace_font_size(),
                        )
                        .with_style(Properties::default().weight(Weight::Bold))
                        .with_color(theme.active_ui_text_color().into())
                        .finish()
                    )
                    .finish()
            )
            .with_padding_left(10.)
            .with_padding_right(10.)
            .with_padding_top(8.)
            .with_padding_bottom(8.)
            .finish()
        );

        let inner_container = Container::new(form_column.finish())
            .with_background(theme.surface_2())
            .with_corner_radius(corner_radius)
            .with_drop_shadow(DropShadow::default())
            .with_border(Border::all(1.0).with_border_fill(theme.outline()))
            .finish();

        let margin = 0.;
        Container::new(inner_container)
            .with_margin_top(12.0)
            .with_margin_left(margin)
            .with_margin_right(margin)
            .finish()
    }
}
