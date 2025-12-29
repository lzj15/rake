use iced::widget::{Column, button, column, pick_list, row, scrollable, slider, text};
use iced::{Element, Length};
use jack::{AudioIn, AudioOut, Client, ClientOptions, ProcessHandler};
use rack::prelude::*;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

fn main() -> iced::Result {
    iced::application(boot, update, view).run()
}

struct LoadedPlugin {
    id: usize,
    info: PluginInfo,
    params: Vec<(ParameterInfo, f32)>,
}

#[derive(Default)]
struct AppState {
    plugin_scanner: Option<Scanner>,
    scanned_plugins: Vec<PluginInfo>,
    loaded_plugins: Vec<LoadedPlugin>,
    // An unique id for each plugin loaded
    plugin_id: usize,
    volume: f32,
    command_sender: Option<HeapProd<Command>>,
    _jack_client: Option<jack::AsyncClient<(), PluginProcessor>>,
}

#[derive(Debug, Clone)]
enum Message {
    Scan,
    LoadPlugin(usize),
    DeletePlugin(usize),
    ParamChange(usize, usize, f32),
    VolumeChange(f32),
}

fn update(state: &mut AppState, message: Message) {
    match message {
        Message::Scan => {
            state.scanned_plugins = state
                .plugin_scanner
                .as_ref()
                .unwrap()
                .scan()
                .expect("Failed to scan plugins");
        }
        Message::LoadPlugin(index) => {
            let mut plugin_instance = state
                .plugin_scanner
                .as_ref()
                .unwrap()
                .load(&state.scanned_plugins[index])
                .expect("Failed to load plugin");

            plugin_instance
                .initialize(48000.0, 2048)
                .expect("Failed to initialize plugin");

            let mut params = Vec::with_capacity(plugin_instance.parameter_count());
            for index in 0..plugin_instance.parameter_count() {
                params.push((
                    plugin_instance.parameter_info(index).unwrap(),
                    plugin_instance.get_parameter(index).unwrap(),
                ));
            }

            let plugin = LoadedPlugin {
                id: state.plugin_id,
                info: state.scanned_plugins[index].clone(),
                params,
            };
            state.loaded_plugins.push(plugin);

            if state
                .command_sender
                .as_mut()
                .unwrap()
                .try_push(Command::Load(plugin_instance, state.plugin_id))
                .is_err()
            {
                println!("Failed to send command");
            }

            state.plugin_id += 1;
        }
        Message::DeletePlugin(id) => {
            state.loaded_plugins.retain(|plugin| plugin.id != id);
            if state
                .command_sender
                .as_mut()
                .unwrap()
                .try_push(Command::Delete(id))
                .is_err()
            {
                println!("Failed to send command");
            }
        }
        Message::ParamChange(plugin_id, param_index, value) => {
            for plugin in &mut state.loaded_plugins {
                if plugin.id == plugin_id {
                    plugin.params[param_index].1 = value
                }
            }
            if state
                .command_sender
                .as_mut()
                .unwrap()
                .try_push(Command::ParamChange(plugin_id, param_index, value))
                .is_err()
            {
                println!("Failed to send command");
            }
        }
        Message::VolumeChange(volume) => {
            state.volume = volume;
            if state
                .command_sender
                .as_mut()
                .unwrap()
                .try_push(Command::VolumeChange(volume))
                .is_err()
            {
                println!("Failed to send command");
            }
        }
    }
}

fn view(state: &AppState) -> Element<'_, Message> {
    let mut scanned_plugins_list: Column<'_, Message> = Column::new();
    for info in &state.scanned_plugins {
        scanned_plugins_list = scanned_plugins_list.push(text(format!("{}", info)));
    }

    let mut plugin_list: Column<'_, Message> = Column::new();
    for plugin in &state.loaded_plugins {
        plugin_list = plugin_list.push(text(plugin.info.name.clone()));

        for param in &plugin.params {
            plugin_list = plugin_list.push(row![
                text(param.0.name.clone()).width(Length::Fixed(100.0)),
                text(format!("{:.2} ", param.1)),
                slider(0.0..=1.0, param.1, |value| {
                    // TODO: denormalize parameter value
                    // For VST3, it seems that min/max in ParameterInfo always gives 0.0 and 1.0
                    // so currently there's no way to denormalize parameter value
                    Message::ParamChange(plugin.id, param.0.index, value)
                })
                .step(0.01),
            ]);
        }
        plugin_list = plugin_list.push(button("Delete").on_press(Message::DeletePlugin(plugin.id)));
    }

    scrollable(column![
        button("Rescan").on_press(Message::Scan),
        scrollable(scanned_plugins_list),
        text("\nPick a plugin to load"),
        pick_list(
            Vec::from_iter(0..state.scanned_plugins.len()),
            None::<usize>,
            Message::LoadPlugin
        ),
        plugin_list,
        row![
            text(format!("Volume: {:?} ", state.volume)),
            slider(0.0..=15.0, state.volume, Message::VolumeChange),
        ]
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

enum Command {
    Load(Plugin, usize),
    Delete(usize),
    ParamChange(usize, usize, f32),
    VolumeChange(f32),
}

struct PluginProcessor {
    left_in: jack::Port<AudioIn>,
    right_in: jack::Port<AudioIn>,
    left_out: jack::Port<AudioOut>,
    right_out: jack::Port<AudioOut>,
    command_receiver: HeapCons<Command>,
    // The plugin, its id, and whether is it marked for delete
    plugins: Vec<(Plugin, usize, bool)>,
    l_vec: Vec<f32>,
    r_vec: Vec<f32>,
    volume: f32,
}

impl ProcessHandler for PluginProcessor {
    fn process(&mut self, _: &jack::Client, scope: &jack::ProcessScope) -> jack::Control {
        match self.command_receiver.try_pop() {
            Some(Command::Load(plugin, id)) => {
                self.plugins.push((plugin, id, false));
            }
            Some(Command::Delete(id)) => {
                // self.plugins.remove(index);
                // Remove the plugin directly will cause panic
                // so mark the plugin for delete
                for index in 0..self.plugins.len() {
                    if self.plugins[index].1 == id {
                        self.plugins[index].2 = true;
                    }
                }
            }
            Some(Command::ParamChange(plugin_id, param_index, value)) => {
                for plugin in &mut self.plugins {
                    if plugin.1 == plugin_id {
                        plugin
                            .0
                            .set_parameter(param_index, value)
                            .expect("Failed to set parameter");
                    }
                }
            }
            Some(Command::VolumeChange(volume)) => {
                self.volume = volume;
            }
            None => (),
        }
        let l_in = self.left_in.as_slice(scope);
        let r_in = self.right_in.as_slice(scope);
        let l_out = self.left_out.as_mut_slice(scope);
        let r_out = self.right_out.as_mut_slice(scope);

        l_out.copy_from_slice(l_in);
        r_out.copy_from_slice(r_in);

        self.l_vec.copy_from_slice(l_in);
        self.r_vec.copy_from_slice(r_in);

        for plugin in &mut self.plugins {
            // Only process plugins not marked for delete
            if plugin.2 == false {
                plugin
                    .0
                    .process(
                        &[self.l_vec.as_mut_slice(), self.r_vec.as_mut_slice()],
                        &mut [l_out, r_out],
                        l_in.len(),
                    )
                    .expect("Plugin failed to process");

                self.l_vec.copy_from_slice(l_out);
                self.r_vec.copy_from_slice(r_out);
            }
        }

        for sample in l_out {
            *sample = *sample * self.volume * self.volume;
        }
        for sample in r_out {
            *sample = *sample * self.volume * self.volume;
        }

        jack::Control::Continue
    }
}

fn boot() -> AppState {
    let (client, _status) = Client::new("rake", ClientOptions::NO_START_SERVER).unwrap();
    let (prod, cons) = HeapRb::<Command>::new(100).split();

    let plugin_processor = PluginProcessor {
        left_in: client.register_port("in_left", AudioIn::default()).unwrap(),
        right_in: client
            .register_port("in_right", AudioIn::default())
            .unwrap(),
        left_out: client
            .register_port("out_left", AudioOut::default())
            .unwrap(),
        right_out: client
            .register_port("out_right", AudioOut::default())
            .unwrap(),
        command_receiver: cons,
        plugins: Vec::new(),
        l_vec: vec![0.0; 128],
        r_vec: vec![0.0; 128],
        volume: 1.0,
    };

    let activate_client = client.activate_async((), plugin_processor).unwrap();

    let input_ports = activate_client
        .as_client()
        .ports(None, None, jack::PortFlags::IS_OUTPUT);
    let output_ports = activate_client
        .as_client()
        .ports(None, None, jack::PortFlags::IS_INPUT);

    let _ = activate_client
        .as_client()
        .connect_ports_by_name(&input_ports[0], &format!("rake:in_left"));
    let _ = activate_client
        .as_client()
        .connect_ports_by_name(&input_ports[0], &format!("rake:in_right"));
    let _ = activate_client
        .as_client()
        .connect_ports_by_name(&format!("rake:out_left"), &output_ports[0]);
    let _ = activate_client
        .as_client()
        .connect_ports_by_name(&format!("rake:out_right"), &output_ports[1]);

    let plugin_scanner = Some(Scanner::new().expect("Failed to create scanner"));
    AppState {
        scanned_plugins: plugin_scanner
            .as_ref()
            .unwrap()
            .scan()
            .expect("Failed to scan plugins"),
        plugin_scanner,
        command_sender: Some(prod),
        _jack_client: Some(activate_client),
        ..AppState::default()
    }
}
