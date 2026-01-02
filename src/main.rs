use iced::{Subscription, Task, window};
use rack::prelude::*;
use rfd::FileDialog;
use ringbuf::{HeapCons, HeapProd, traits::Producer};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

mod processor;
mod view;
use processor::*;

fn main() -> iced::Result {
    iced::application(boot, update, view::view)
        .exit_on_close_request(false)
        .subscription(subscription)
        .theme(iced::Theme::CatppuccinLatte)
        .run()
}

fn subscription(_state: &AppState) -> Subscription<Message> {
    window::close_requests().map(|_id| Message::Exit)
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
    loaded_plugins: Vec<LoadedPlugin>,
    volume: f32,
    command_sender: Option<HeapProd<Command>>,
    _garbage_receiver: Option<HeapCons<(Plugin, Uuid)>>,
    session_path: PathBuf,
    jack_client: Option<jack::AsyncClient<(), processor::Processor>>,
}

#[derive(Debug, Clone)]
enum Message {
    Scan,
    LoadPlugin(PluginInfo),
    DeletePlugin(Uuid),
    MovePluginUp(Uuid),
    MovePluginDown(Uuid),
    ParamChange(Uuid, ParameterInfo, f32),
    ClearSession,
    SaveSession,
    LoadSession,
    VolumeChange(f32),
    Exit,
}

fn create_instance(scanner: &Scanner, info: &PluginInfo, client: &jack::Client) -> Result<Plugin> {
    let mut plugin_instance = scanner.load(info)?;
    let _ =
        plugin_instance.initialize(client.sample_rate() as f64, client.buffer_size() as usize)?;
    Ok(plugin_instance)
}

fn load_session(state: &mut AppState, path: &std::path::PathBuf) -> Result<Vec<LoadedPlugin>> {
    let content = std::fs::read_to_string(path)?;
    let mut saved_plugins = serde_yaml_ng::from_str::<Vec<LoadedPlugin>>(&content)
        .map_err(|e| rack::Error::Other(format!("Incorrect YAML: {}", e)))?;

    for plugin in &mut saved_plugins {
        plugin.id = Uuid::new_v4();
    }

    let _ = state
        .command_sender
        .as_mut()
        .unwrap()
        .try_push(Command::ClearSession)
        .map_err(|_| rack::Error::Other("Error sending command to clear session".to_string()))?;

    for plugin in &saved_plugins {
        let plugin_instance = create_instance(
            state.plugin_scanner.as_ref().unwrap(),
            &plugin.info,
            state.jack_client.as_ref().unwrap().as_client(),
        )?;

        let _ = state
            .command_sender
            .as_mut()
            .unwrap()
            .try_push(Command::LoadPlugin(plugin_instance, plugin.id))
            .map_err(|_| rack::Error::Other(format!("Error sending plugin {}", plugin.info)))?;

        for param in &plugin.params {
            let _ = state
                .command_sender
                .as_mut()
                .unwrap()
                .try_push(Command::ParamChange(plugin.id, param.0.clone(), param.1))
                .map_err(|_| {
                    rack::Error::Other(format!(
                        "Error sending parameter {} of {}",
                        param.0.name, plugin.info
                    ))
                })?;
        }
    }
    Ok(saved_plugins)
}

fn update(state: &mut AppState, message: Message) -> Task<Message> {
    match message {
        Message::Scan => {
            match state.plugin_scanner.as_ref().unwrap().scan() {
                Ok(plugins) => {
                    state.scanned_plugins = plugins;
                }
                Err(e) => {
                    eprintln!("Error scanning plugins: {}", e);
                }
            }
            Task::none()
        }
        Message::LoadPlugin(info) => {
            if let Ok(plugin_instance) = create_instance(
                state.plugin_scanner.as_ref().unwrap(),
                &info,
                state.jack_client.as_ref().unwrap().as_client(),
            ) {
                let mut params = Vec::with_capacity(plugin_instance.parameter_count());
                for i in 0..plugin_instance.parameter_count() {
                    params.push((
                        plugin_instance.parameter_info(i).unwrap(),
                        plugin_instance.get_parameter(i).unwrap(),
                    ));
                }

                let plugin = LoadedPlugin {
                    id: Uuid::new_v4(),
                    info: info.clone(),
                    params,
                };

                match state
                    .command_sender
                    .as_mut()
                    .unwrap()
                    .try_push(Command::LoadPlugin(plugin_instance, plugin.id))
                {
                    Ok(_) => {
                        state.loaded_plugins.push(plugin);
                    }

                    Err(_) => {
                        eprintln!("Error sending plugin: {}", info);
                    }
                }
            }
            Task::none()
        }
        Message::DeletePlugin(id) => {
            match state
                .command_sender
                .as_mut()
                .unwrap()
                .try_push(Command::DeletePlugin(id))
            {
                Ok(_) => {
                    state.loaded_plugins.retain(|plugin| plugin.id != id);
                }
                Err(_) => {
                    eprintln!("Error sending command to delete plugin");
                }
            }
            // TODO: Safely drop plugin instances
            // if let Some(_plugin) = state.garbage_receiver.as_mut().unwrap().try_pop() {
            // }
            Task::none()
        }
        Message::MovePluginUp(id) => {
            match state
                .command_sender
                .as_mut()
                .unwrap()
                .try_push(Command::MovePluginUp(id))
            {
                Ok(_) => {
                    if let Some(i) = state
                        .loaded_plugins
                        .iter()
                        .position(|plugin| plugin.id == id)
                    {
                        state.loaded_plugins.swap(i - 1, i);
                    }
                }
                Err(_) => {
                    eprintln!("Error sending command to move plugin up");
                }
            }
            Task::none()
        }
        Message::MovePluginDown(id) => {
            match state
                .command_sender
                .as_mut()
                .unwrap()
                .try_push(Command::MovePluginDown(id))
            {
                Ok(_) => {
                    if let Some(i) = state
                        .loaded_plugins
                        .iter()
                        .rposition(|plugin| plugin.id == id)
                    {
                        state.loaded_plugins.swap(i, i + 1);
                    }
                }
                Err(_) => {
                    eprintln!("Error sending command to move plugin down");
                }
            }
            Task::none()
        }
        Message::ParamChange(plugin_id, param_info, value) => {
            match state
                .command_sender
                .as_mut()
                .unwrap()
                .try_push(Command::ParamChange(plugin_id, param_info.clone(), value))
            {
                Ok(_) => {
                    if let Some(plugin) = state
                        .loaded_plugins
                        .iter_mut()
                        .find(|plugin| plugin.id == plugin_id)
                    {
                        plugin.params[param_info.index].1 = value
                    }
                }
                Err(_) => {
                    eprintln!("Error sending parameter {}", param_info.name);
                }
            }
            Task::none()
        }
        Message::ClearSession => {
            match state
                .command_sender
                .as_mut()
                .unwrap()
                .try_push(Command::ClearSession)
            {
                Ok(_) => {
                    state.loaded_plugins.clear();
                }
                Err(_) => {
                    eprintln!("Error sending command to clear session");
                }
            }
            Task::none()
        }
        Message::SaveSession => {
            let content = serde_yaml_ng::to_string(&state.loaded_plugins).unwrap();
            if state.session_path.exists() {
                if let Err(e) = std::fs::write(&state.session_path, content) {
                    eprintln!("Error writing {}: {}", state.session_path.display(), e);
                }
            } else {
                if let Some(path) = FileDialog::new()
                    .add_filter("YAML", &["yaml"])
                    .set_file_name(".yaml")
                    .save_file()
                {
                    if let Err(e) = std::fs::write(&path, content) {
                        eprintln!("Error writing {}: {}", path.display(), e);
                    }
                }
            }
            Task::none()
        }
        Message::LoadSession => {
            if let Some(path) = FileDialog::new()
                .set_directory(state.session_path.parent().unwrap_or_else(|| Path::new("")))
                .add_filter("YAML", &["yaml"])
                .pick_file()
            {
                match load_session(state, &path) {
                    Ok(plugins) => {
                        state.loaded_plugins = plugins;
                        state.session_path = path;
                    }
                    Err(e) => {
                        eprintln!("Error loading {}: {}", path.display(), e)
                    }
                }
            }
            Task::none()
        }
        Message::VolumeChange(volume) => {
            match state
                .command_sender
                .as_mut()
                .unwrap()
                .try_push(Command::VolumeChange(volume))
            {
                Ok(_) => {
                    state.volume = volume;
                }
                Err(_) => {
                    eprintln!("Error sending command to change volume");
                }
            }
            Task::none()
        }
        Message::Exit => {
            let _ = state
                .command_sender
                .as_mut()
                .unwrap()
                .try_push(Command::Exit);
            iced::exit()
        }
    }
}

fn boot() -> AppState {
    let (active_client, command_sender, garbage_receiver) = processor::initialize();
    let plugin_scanner = Some(Scanner::new().expect("Error creating plugin scanner"));
    AppState {
        scanned_plugins: plugin_scanner.as_ref().unwrap().scan().unwrap_or_else(|e| {
            eprintln!("Error scanning plugins: {}", e);
            Vec::new()
        }),
        plugin_scanner,
        volume: 1.0,
        command_sender: Some(command_sender),
        _garbage_receiver: Some(garbage_receiver),
        jack_client: Some(active_client),
        ..AppState::default()
    }
}
