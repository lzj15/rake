use iced::widget::{Column, Row, button, column, row, scrollable, slider, text};
use iced::{Element, Length};
use jack::{AudioIn, AudioOut, Client, ClientOptions, ProcessHandler};
use rack::prelude::*;
use rfd::FileDialog;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn main() -> iced::Result {
    iced::application(boot, update, view).run()
}

#[derive(Serialize, Deserialize)]
struct LoadedPlugin {
    id: Uuid,
    info: PluginInfo,
    params: Vec<(ParameterInfo, f32)>,
}

#[derive(Default)]
struct AppState {
    plugin_scanner: Option<Scanner>,
    scanned_plugins: Vec<PluginInfo>,
    added_plugins: Vec<LoadedPlugin>,
    volume: f32,
    command_sender: Option<HeapProd<Command>>,
    _jack_client: Option<jack::AsyncClient<(), PluginProcessor>>,
}

#[derive(Debug, Clone)]
enum Message {
    Scan,
    AddPlugin(String),
    DeletePlugin(Uuid),
    MovePluginUp(Uuid),
    MovePluginDown(Uuid),
    ParamChange(Uuid, usize, f32),
    VolumeChange(f32),
    SaveState,
    LoadState,
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
        Message::AddPlugin(id) => {
            for info in &state.scanned_plugins {
                if info.unique_id == id {
                    let mut plugin_instance = state
                        .plugin_scanner
                        .as_ref()
                        .unwrap()
                        .load(&info)
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
                        id: Uuid::new_v4(),
                        info: info.clone(),
                        params,
                    };

                    if state
                        .command_sender
                        .as_mut()
                        .unwrap()
                        .try_push(Command::LoadPlugin(plugin_instance, plugin.id))
                        .is_err()
                    {
                        eprintln!("Failed to send command");
                    }
                    state.added_plugins.push(plugin);
                }
            }
        }
        Message::DeletePlugin(id) => {
            state.added_plugins.retain(|plugin| plugin.id != id);
            if state
                .command_sender
                .as_mut()
                .unwrap()
                .try_push(Command::DeletePlugin(id))
                .is_err()
            {
                eprintln!("Failed to send command");
            }
        }
        Message::MovePluginUp(id) => {
            let index = state
                .added_plugins
                .iter()
                .position(|plugin| plugin.id == id);
            if let Some(i) = index {
                state.added_plugins.swap(i - 1, i);
                if state
                    .command_sender
                    .as_mut()
                    .unwrap()
                    .try_push(Command::MovePluginUp(id))
                    .is_err()
                {
                    eprintln!("Failed to send command");
                }
            }
        }
        Message::MovePluginDown(id) => {
            let index = state
                .added_plugins
                .iter()
                .position(|plugin| plugin.id == id);
            if let Some(i) = index {
                state.added_plugins.swap(i, i + 1);
                if state
                    .command_sender
                    .as_mut()
                    .unwrap()
                    .try_push(Command::MovePluginDown(id))
                    .is_err()
                {
                    eprintln!("Failed to send command");
                }
            }
        }
        Message::ParamChange(plugin_id, param_index, value) => {
            for plugin in &mut state.added_plugins {
                if plugin.id == plugin_id {
                    plugin.params[param_index].1 = value;
                }
            }
            if state
                .command_sender
                .as_mut()
                .unwrap()
                .try_push(Command::ParamChange(plugin_id, param_index, value))
                .is_err()
            {
                eprintln!("Failed to send command");
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
                eprintln!("Failed to send command");
            }
        }
        Message::SaveState => {
            if let Some(path) = FileDialog::new()
                .add_filter("YAML", &["yaml"])
                .set_file_name(".yaml")
                .save_file()
            {
                let content = serde_yaml_ng::to_string(&state.added_plugins).unwrap();
                std::fs::write(path, content).expect("Failed to write file");
            }
        }
        Message::LoadState => {
            if let Some(path) = FileDialog::new().add_filter("YAML", &["yaml"]).pick_file() {
                let content = std::fs::read_to_string(path).unwrap();
                let mut added_plugins = serde_yaml_ng::from_str::<Vec<LoadedPlugin>>(&content)
                    .expect("Failed to read yaml");

                for plugin in &mut added_plugins {
                    plugin.id = Uuid::new_v4();
                }
                state.added_plugins = added_plugins;

                if state
                    .command_sender
                    .as_mut()
                    .unwrap()
                    .try_push(Command::ClearEnabledPlugins)
                    .is_err()
                {
                    eprintln!("Failed to send command");
                }

                for plugin in &state.added_plugins {
                    let mut plugin_instance = state
                        .plugin_scanner
                        .as_ref()
                        .unwrap()
                        .load(&plugin.info)
                        .expect("Failed to load plugin");

                    plugin_instance
                        .initialize(48000.0, 2048)
                        .expect("Failed to initialize plugin");

                    if state
                        .command_sender
                        .as_mut()
                        .unwrap()
                        .try_push(Command::LoadPlugin(plugin_instance, plugin.id))
                        .is_err()
                    {
                        eprintln!("Failed to send command");
                    }

                    for param in &plugin.params {
                        if state
                            .command_sender
                            .as_mut()
                            .unwrap()
                            .try_push(Command::ParamChange(plugin.id, param.0.index, param.1))
                            .is_err()
                        {
                            eprintln!("Failed to send command");
                        }
                    }
                }
            }
        }
    }
}

fn view(state: &AppState) -> Element<'_, Message> {
    let mut scanned_plugins_list = Column::new();
    for info in &state.scanned_plugins {
        scanned_plugins_list = scanned_plugins_list.push(row![
            button("Load").on_press(Message::AddPlugin(info.unique_id.clone())),
            text(format!(" {}", info))
        ]);
    }

    let mut plugin_list = Column::new();
    for (index, plugin) in state.added_plugins.iter().enumerate() {
        for param in &plugin.params {
            plugin_list = plugin_list.push(row![
                text(param.0.name.clone()).width(Length::Fixed(100.0)),
                text(format!("{:.2} ", param.1)),
                slider(0.0..=1.0, param.1, move |value| {
                    Message::ParamChange(plugin.id, param.0.index, value)
                })
                .step(0.01),
            ]);
        }

        let mut move_controls = Row::new();
        if index != 0 {
            move_controls =
                move_controls.push(button("Up").on_press(Message::MovePluginUp(plugin.id)));
        }
        if index != state.added_plugins.len() - 1 {
            move_controls =
                move_controls.push(button("Down").on_press(Message::MovePluginDown(plugin.id)));
        }

        plugin_list = plugin_list.push(move_controls);
        plugin_list = plugin_list.push(button("Delete").on_press(Message::DeletePlugin(plugin.id)));
    }

    scrollable(column![
        button("Rescan").on_press(Message::Scan),
        scanned_plugins_list,
        plugin_list,
        row![
            button("Save").on_press(Message::SaveState),
            button("Open").on_press(Message::LoadState)
        ],
        row![
            text(format!("Volume: {:.2} ", state.volume)),
            slider(0.0..=10.0, state.volume, Message::VolumeChange).step(0.01),
        ]
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

enum Command {
    LoadPlugin(Plugin, Uuid),
    DeletePlugin(Uuid),
    MovePluginUp(Uuid),
    MovePluginDown(Uuid),
    ParamChange(Uuid, usize, f32),
    VolumeChange(f32),
    ClearEnabledPlugins,
}

struct PluginProcessor {
    left_in: jack::Port<AudioIn>,
    right_in: jack::Port<AudioIn>,
    left_out: jack::Port<AudioOut>,
    right_out: jack::Port<AudioOut>,
    command_receiver: HeapCons<Command>,
    plugin_instances: Vec<(Plugin, Uuid)>,
    enabled_plugins: Vec<Uuid>,
    l_vec: Vec<f32>,
    r_vec: Vec<f32>,
    volume: f32,
}

impl ProcessHandler for PluginProcessor {
    fn process(&mut self, client: &jack::Client, scope: &jack::ProcessScope) -> jack::Control {
        while let Some(command) = self.command_receiver.try_pop() {
            match command {
                Command::LoadPlugin(plugin, id) => {
                    self.plugin_instances.push((plugin, id));
                    self.enabled_plugins.push(id);
                }
                Command::DeletePlugin(id) => {
                    self.enabled_plugins.retain(|plugin_id| *plugin_id != id);
                }
                Command::MovePluginUp(id) => {
                    if let Some(index) =
                        self.enabled_plugins.iter().position(|plugin| *plugin == id)
                    {
                        self.enabled_plugins.swap(index - 1, index);
                    }
                }
                Command::MovePluginDown(id) => {
                    if let Some(index) = self
                        .enabled_plugins
                        .iter()
                        .rposition(|plugin| *plugin == id)
                    {
                        self.enabled_plugins.swap(index, index + 1);
                    }
                }
                Command::ParamChange(plugin_id, param_index, value) => {
                    for plugin in &mut self.plugin_instances {
                        if plugin.1 == plugin_id {
                            let _ = plugin.0.set_parameter(param_index, value);
                        }
                    }
                }
                Command::VolumeChange(volume) => {
                    self.volume = volume;
                }
                Command::ClearEnabledPlugins => {
                    self.enabled_plugins.clear();
                }
            }
        }

        let l_in = self.left_in.as_slice(scope);
        let r_in = self.right_in.as_slice(scope);
        let l_out = self.left_out.as_mut_slice(scope);
        let r_out = self.right_out.as_mut_slice(scope);

        l_out.copy_from_slice(l_in);
        r_out.copy_from_slice(r_in);
        self.l_vec.copy_from_slice(l_in);
        self.r_vec.copy_from_slice(r_in);

        for id in &self.enabled_plugins {
            if let Some(plugin) = self
                .plugin_instances
                .iter_mut()
                .find(|plugin| plugin.1 == *id)
            {
                let _ = plugin.0.process(
                    &[self.l_vec.as_mut_slice(), self.r_vec.as_mut_slice()],
                    &mut [l_out, r_out],
                    client.buffer_size() as usize,
                );
                self.l_vec.copy_from_slice(l_out);
                self.r_vec.copy_from_slice(r_out);
            }
        }

        for sample in l_out.iter_mut() {
            *sample *= self.volume * self.volume;
        }
        for sample in r_out.iter_mut() {
            *sample *= self.volume * self.volume;
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
        plugin_instances: Vec::new(),
        enabled_plugins: Vec::new(),
        l_vec: vec![0.0; client.buffer_size() as usize],
        r_vec: vec![0.0; client.buffer_size() as usize],
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
