use iced::Element;
use iced::widget::{button, column, pick_list, slider, text};
use jack::{AudioIn, AudioOut, Client, ClientOptions, ProcessHandler};
use rack::prelude::*;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

fn main() -> iced::Result {
    iced::application(boot, update, view).run()
}

#[derive(Default)]
struct AppState {
    plugin_scanner: Option<Scanner>,
    scanned_plugins: Vec<PluginInfo>,
    // Plugin's info and its id
    loaded_plugins: Vec<(PluginInfo, usize)>,
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
    VolumeChange(f32),
}

fn update(state: &mut AppState, message: Message) {
    match message {
        Message::Scan => {
            state.plugin_scanner = Some(Scanner::new().expect("Failed to create plugin scanner"));
            state.scanned_plugins = state
                .plugin_scanner
                .as_ref()
                .unwrap()
                .scan()
                .expect("Failed to scan plugins");
        }
        Message::LoadPlugin(index) => {
            state
                .loaded_plugins
                .push((state.scanned_plugins[index].clone(), state.plugin_id));

            let mut plugin = state
                .plugin_scanner
                .as_ref()
                .expect("Empty scanner")
                .load(&state.scanned_plugins[index])
                .expect("Load fail");

            plugin.initialize(48000.0, 2048).unwrap();

            if state
                .command_sender
                .as_mut()
                .expect("empty sender")
                .try_push(Command::Load(plugin, state.plugin_id))
                .is_err()
            {
                println!("Failed to send command");
            }

            state.plugin_id += 1;
        }
        Message::DeletePlugin(id) => {
            for index in 0..state.loaded_plugins.len() {
                if state.loaded_plugins[index].1 == id {
                    state.loaded_plugins.remove(index);
                }
            }
            if state
                .command_sender
                .as_mut()
                .expect("empty sender")
                .try_push(Command::Delete(id))
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
                .expect("empty sender")
                .try_push(Command::VolumeChange(volume))
                .is_err()
            {
                println!("Failed to send command");
            }
        }
    }
}

fn view(state: &AppState) -> Element<'_, Message> {
    column![
        button("Scan").on_press(Message::Scan),
        text(format!("Scanned plugins:\n\n{:?}\n", state.scanned_plugins)),
        text("Pick a plugin to load"),
        pick_list(
            Vec::from_iter(0..state.scanned_plugins.len()),
            None::<usize>,
            Message::LoadPlugin
        ),
        text(format!("Loaded plugins:\n\n{:?}\n", state.loaded_plugins)),
        text("Pick a plugin to delete"),
        pick_list(
            state
                .loaded_plugins
                .iter()
                .map(|&(_, id)| id)
                .collect::<Vec<usize>>(),
            // Vec::from_iter(0..state.loaded_plugins.len()),
            None::<usize>,
            Message::DeletePlugin
        ),
        text(format!("Volume:{:?}", state.volume)),
        slider(0.0..=15.0, state.volume, Message::VolumeChange),
    ]
    .into()
}

enum Command {
    Load(Plugin, usize),
    Delete(usize),
    VolumeChange(f32),
}

struct PluginProcessor {
    left_in: jack::Port<AudioIn>,
    right_in: jack::Port<AudioIn>,
    left_out: jack::Port<AudioOut>,
    right_out: jack::Port<AudioOut>,
    command_receiver: HeapCons<Command>,
    // The plugin, is it marked for delete, and its id
    plugins: Vec<(Plugin, bool, usize)>,
    l_vec: Vec<f32>,
    r_vec: Vec<f32>,
    volume: f32,
}

impl ProcessHandler for PluginProcessor {
    fn process(&mut self, _: &jack::Client, scope: &jack::ProcessScope) -> jack::Control {
        match self.command_receiver.try_pop() {
            Some(Command::VolumeChange(volume)) => {
                self.volume = volume;
            }
            Some(Command::Load(plugin, id)) => {
                self.plugins.push((plugin, false, id));
            }
            Some(Command::Delete(id)) => {
                // self.plugins.remove(index);
                // Remove the plugin directly will panic
                // so mark the plugin for delete
                for index in 0..self.plugins.len() {
                    if self.plugins[index].2 == id {
                        self.plugins[index].1 = true;
                    }
                }
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
            if plugin.1 == false {
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

    AppState {
        command_sender: Some(prod),
        _jack_client: Some(activate_client),
        ..AppState::default()
    }
}
