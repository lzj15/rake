use crate::Message;
use iced::widget::{Column, Row, button, column, container, row, scrollable, slider, space, text};
use iced::{Alignment, Color, Element, Length, Theme};

pub fn view(state: &crate::AppState) -> Element<'_, Message> {
    let toolbar = row![
        button("Open").on_press(Message::LoadSession),
        button("Save").on_press(Message::SaveSession),
        button("Clear").on_press(Message::ClearSession),
        button("Rescan").on_press(Message::Scan),
        space::horizontal().width(6),
        text(format!(
            "{}",
            state
                .session_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        ))
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let mut scanned_list = column![].spacing(10);
    for info in &state.scanned_plugins {
        scanned_list = scanned_list.push(
            container(
                row![
                    text(format!("{}", info)).width(223.0),
                    button("+").on_press(Message::LoadPlugin(info.clone())),
                ]
                .spacing(10)
                .padding(10)
                .align_y(Alignment::Center),
            )
            .style(box_style),
        );
    }

    let mut plugin_chain = column![].spacing(15);
    for (i, plugin) in state.loaded_plugins.iter().enumerate() {
        let mut plugin_header: Row<'_, Message> = row![].spacing(10).align_y(Alignment::Center);
        plugin_header = plugin_header.push(text(&plugin.info.name));
        plugin_header = plugin_header.push(button("✕").on_press(Message::DeletePlugin(plugin.id)));

        if i != 0 {
            plugin_header =
                plugin_header.push(button("↑").on_press(Message::MovePluginUp(plugin.id)));
        }
        if i != state.loaded_plugins.len() - 1 {
            plugin_header =
                plugin_header.push(button("↓").on_press(Message::MovePluginDown(plugin.id)));
        }

        let mut param_controls: Column<'_, Message> = column![].spacing(10);
        for param in &plugin.params {
            param_controls = param_controls.push(row![
                text(param.0.name.clone()).width(100.0),
                text(format!("{:.2} ", param.1)),
                slider(0.0..=1.0, param.1, |value|
                    // TODO: denormalize parameter value
                    // For VST3, it seems that min & max in ParameterInfo always gives 0.0 and 1.0
                    // so currently there's no way to denormalize parameter value
                    Message::ParamChange(
                    plugin.id,
                    param.0.clone(),
                    value
                ))
                .step(0.01),
            ]);
        }

        plugin_chain = plugin_chain.push(
            container(
                column![plugin_header, param_controls]
                    .spacing(15)
                    .padding(15),
            )
            .style(box_style),
        );
    }

    container(
        column![
            toolbar,
            row![
                text(" Available").color([0.5, 0.5, 0.5]),
                space::horizontal().width(233),
                text("Active Chain").color([0.5, 0.5, 0.5]),
            ],
            row![
                scrollable(scanned_list).spacing(8),
                scrollable(plugin_chain).spacing(8),
            ]
            .spacing(20)
            .height(Length::Fill),
            row![
                text(format!("Master Volume: {:.2} ", state.volume)),
                slider(0.0..=5.0, state.volume, Message::VolumeChange).step(0.01),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(15)
        .padding(20),
    )
    .style(|_theme: &Theme| container::Style {
        background: Some(Color::from_rgb8(240, 240, 240).into()),
        ..Default::default()
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn box_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::WHITE.into()),
        border: iced::Border {
            radius: 10.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}
