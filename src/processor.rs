use jack::{AudioIn, AudioOut, Client, ClientOptions, ProcessHandler};
use rack::prelude::*;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use uuid::Uuid;

pub enum Command {
    LoadPlugin(Plugin, Uuid),
    DeletePlugin(Uuid),
    MovePluginUp(Uuid),
    MovePluginDown(Uuid),
    ParamChange(Uuid, ParameterInfo, f32),
    ClearSession,
    VolumeChange(f32),
    Exit,
}

pub struct Processor {
    left_in: jack::Port<AudioIn>,
    right_in: jack::Port<AudioIn>,
    left_out: jack::Port<AudioOut>,
    right_out: jack::Port<AudioOut>,
    loaded_plugins: Vec<(Plugin, Uuid)>,
    volume: f32,
    command_receiver: HeapCons<Command>,
    garbage_sender: HeapProd<(Plugin, Uuid)>,
    l_vec: Vec<f32>,
    r_vec: Vec<f32>,
}

impl ProcessHandler for Processor {
    fn process(&mut self, client: &jack::Client, scope: &jack::ProcessScope) -> jack::Control {
        match self.command_receiver.try_pop() {
            Some(Command::LoadPlugin(plugin, id)) => {
                self.loaded_plugins.push((plugin, id));
            }
            Some(Command::DeletePlugin(id)) => {
                if let Some(i) = self
                    .loaded_plugins
                    .iter()
                    .rposition(|plugin| plugin.1 == id)
                {
                    if let Err(e) = self.garbage_sender.try_push(self.loaded_plugins.remove(i)) {
                        eprintln!("Error removing plugin {}", e.0.info())
                    }
                }
            }
            Some(Command::MovePluginUp(id)) => {
                if let Some(i) = self.loaded_plugins.iter().position(|plugin| plugin.1 == id) {
                    self.loaded_plugins.swap(i - 1, i);
                }
            }
            Some(Command::MovePluginDown(id)) => {
                if let Some(i) = self
                    .loaded_plugins
                    .iter()
                    .rposition(|plugin| plugin.1 == id)
                {
                    self.loaded_plugins.swap(i, i + 1);
                }
            }
            Some(Command::ParamChange(plugin_id, param_info, value)) => {
                if let Some(plugin) = self
                    .loaded_plugins
                    .iter_mut()
                    .find(|plugin| plugin.1 == plugin_id)
                {
                    if let Err(e) = plugin.0.set_parameter(param_info.index, value) {
                        eprintln!(
                            "Error setting parameter {} of {}: {}",
                            param_info.name,
                            plugin.0.info(),
                            e
                        )
                    }
                }
            }
            Some(Command::ClearSession) => {
                for i in (0..self.loaded_plugins.len()).rev() {
                    if let Err(e) = self.garbage_sender.try_push(self.loaded_plugins.remove(i)) {
                        eprintln!("Error removing plugin {}", e.0.info())
                    }
                }
            }
            Some(Command::VolumeChange(volume)) => {
                self.volume = volume;
            }
            Some(Command::Exit) => {
                return jack::Control::Quit;
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

        for plugin in &mut self.loaded_plugins {
            match plugin.0.process(
                &[self.l_vec.as_mut_slice(), self.r_vec.as_mut_slice()],
                &mut [l_out, r_out],
                client.buffer_size() as usize,
            ) {
                Ok(_) => {
                    self.l_vec.copy_from_slice(l_out);
                    self.r_vec.copy_from_slice(r_out);
                }
                Err(e) => {
                    eprintln!("Plugin {} failed to process: {}", plugin.0.info(), e)
                }
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

pub fn initialize() -> (
    jack::AsyncClient<(), Processor>,
    HeapProd<Command>,
    HeapCons<(Plugin, Uuid)>,
) {
    let (client, _status) = Client::new("Rake", ClientOptions::NO_START_SERVER).unwrap();
    let (command_sender, command_receiver) = HeapRb::<Command>::new(512).split();
    let (garbage_sender, garbage_receiver) = HeapRb::<(Plugin, Uuid)>::new(128).split();

    let plugin_processor = Processor {
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
        loaded_plugins: Vec::new(),
        volume: 1.0,
        command_receiver,
        garbage_sender,
        l_vec: vec![0.0; client.buffer_size() as usize],
        r_vec: vec![0.0; client.buffer_size() as usize],
    };

    let active_client = client.activate_async((), plugin_processor).unwrap();

    let input_ports = active_client
        .as_client()
        .ports(None, None, jack::PortFlags::IS_OUTPUT);
    let output_ports = active_client
        .as_client()
        .ports(None, None, jack::PortFlags::IS_INPUT);

    let _ = active_client
        .as_client()
        .connect_ports_by_name(&input_ports[0], &format!("Rake:in_left"));
    let _ = active_client
        .as_client()
        .connect_ports_by_name(&input_ports[0], &format!("Rake:in_right"));
    let _ = active_client
        .as_client()
        .connect_ports_by_name(&format!("Rake:out_left"), &output_ports[0]);
    let _ = active_client
        .as_client()
        .connect_ports_by_name(&format!("Rake:out_right"), &output_ports[1]);

    (active_client, command_sender, garbage_receiver)
}
