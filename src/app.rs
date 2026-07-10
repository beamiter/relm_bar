use chrono::Local;
use gtk::glib::{self, ControlFlow};
use gtk::prelude::*;
use gtk4 as gtk;
use log::{error, info, warn};
use relm4::{
    Component, ComponentController, ComponentParts, ComponentSender, Controller, SimpleComponent,
};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use shared_structures::{CommandType, SharedCommand, SharedMessage, SharedRingBuffer, TagStatus};
use xbar_core::audio_manager::AudioManager;
use xbar_core::system_monitor::SystemMonitor;

use crate::components::{
    LayoutSelectorInput, LayoutSelectorModel, LayoutSelectorOutput, LayoutSelectorState,
    StatusStripInput, StatusStripModel, StatusStripOutput, StatusStripState, WorkspacesInput,
    WorkspacesModel, WorkspacesOutput, WorkspacesState,
};

#[derive(Debug)]
pub enum AppInput {
    TabSelected(usize),
    LayoutChanged(u32),
    ToggleLayoutPanel,
    ToggleSeconds,
    ToggleTheme,
    ToggleMute,
    VolumeStep(i32),
    Screenshot,
    SharedMessageReceived(SharedMessage),
    SystemUpdate,
    UpdateTime,
}

pub struct AppModel {
    active_tab: usize,
    layout_symbol: String,
    layout_open: bool,
    monitor_num: u8,
    show_seconds: bool,
    theme_dark: bool,
    tag_status_vec: Vec<TagStatus>,
    current_time: String,
    current_volume: Option<i32>,
    current_muted: bool,
    cpu_usage: f64,
    memory_usage: f64,

    shared_buffer_opt: Option<Arc<SharedRingBuffer>>,
    audio_manager: AudioManager,
    system_monitor: SystemMonitor,

    root_window: gtk::ApplicationWindow,
    workspaces: Controller<WorkspacesModel>,
    layout_selector: Controller<LayoutSelectorModel>,
    status_strip: Controller<StatusStripModel>,
}

#[relm4::component(pub)]
impl SimpleComponent for AppModel {
    type Init = String;
    type Input = AppInput;
    type Output = ();

    view! {
        #[root]
        gtk::ApplicationWindow {
            set_title: Some("relm_bar"),
            set_decorated: false,
            set_default_size: (1000, 40),
            set_resizable: true,
            add_css_class: "transparent-window",
            set_child: Some(&top_hbox),
        }
    }

    fn init(
        shared_path: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        crate::components::load_css();

        let workspaces = WorkspacesModel::builder()
            .launch(WorkspacesState::default())
            .forward(sender.input_sender(), |output| match output {
                WorkspacesOutput::Selected(index) => AppInput::TabSelected(index),
            });

        let layout_selector = LayoutSelectorModel::builder()
            .launch(LayoutSelectorState::default())
            .forward(sender.input_sender(), |output| match output {
                LayoutSelectorOutput::TogglePanel => AppInput::ToggleLayoutPanel,
                LayoutSelectorOutput::SelectLayout(index) => AppInput::LayoutChanged(index),
            });

        let status_strip = StatusStripModel::builder()
            .launch(StatusStripState::default())
            .forward(sender.input_sender(), |output| match output {
                StatusStripOutput::ToggleSeconds => AppInput::ToggleSeconds,
                StatusStripOutput::ToggleTheme => AppInput::ToggleTheme,
                StatusStripOutput::ToggleMute => AppInput::ToggleMute,
                StatusStripOutput::VolumeStep(step) => AppInput::VolumeStep(step),
                StatusStripOutput::Screenshot => AppInput::Screenshot,
            });

        let top_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        top_hbox.set_margin_top(0);
        top_hbox.set_margin_bottom(0);
        top_hbox.set_margin_start(1);
        top_hbox.set_margin_end(1);
        top_hbox.add_css_class("panel-root");

        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);

        top_hbox.append(workspaces.widget());
        top_hbox.append(layout_selector.widget());
        top_hbox.append(&spacer);
        top_hbox.append(status_strip.widget());

        root.set_title(Some("relm_bar"));

        root.connect_realize(|window| {
            window.set_title(Some("relm_bar"));
            if let Some(surface) = window.surface() {
                surface.set_opaque_region(None);
            }
            window.add_css_class("transparent-window");
            window.queue_draw();
        });

        let shared_buffer_opt =
            SharedRingBuffer::create_shared_ring_buffer_aux(&shared_path).map(Arc::new);

        let mut model = AppModel {
            active_tab: 0,
            layout_symbol: " ? ".to_string(),
            layout_open: false,
            monitor_num: 0,
            show_seconds: false,
            theme_dark: true,
            tag_status_vec: Vec::new(),
            current_time: String::new(),
            current_volume: None,
            current_muted: false,
            cpu_usage: 0.0,
            memory_usage: 0.0,
            shared_buffer_opt: shared_buffer_opt.clone(),
            audio_manager: AudioManager::new(),
            system_monitor: SystemMonitor::new(10),
            root_window: root.clone(),
            workspaces,
            layout_selector,
            status_strip,
        };

        root.add_css_class("theme-dark");
        root.remove_css_class("theme-light");

        model.update_time_display();
        model.refresh_volume_state();
        model.sync_all_views();

        spawn_background_tasks(sender.clone(), shared_buffer_opt);
        sender.input(AppInput::SystemUpdate);

        // Present window to ensure proper initialization
        root.present();

        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            AppInput::TabSelected(index) => {
                info!("Tab selected: {}", index);
                self.active_tab = index;
                self.send_tag_command(true);
                self.sync_workspaces_view();
            }
            AppInput::LayoutChanged(layout_index) => {
                info!("Layout changed: {}", layout_index);
                self.send_layout_command(layout_index);
                self.layout_open = false;
                self.sync_layout_view();
            }
            AppInput::ToggleLayoutPanel => {
                self.layout_open = !self.layout_open;
                self.sync_layout_view();
            }
            AppInput::ToggleSeconds => {
                self.show_seconds = !self.show_seconds;
                self.update_time_display();
                self.sync_status_view();
            }
            AppInput::ToggleTheme => {
                self.theme_dark = !self.theme_dark;
                self.sync_root_theme();
                self.sync_status_view();
            }
            AppInput::ToggleMute => {
                self.toggle_mute();
                self.sync_status_view();
            }
            AppInput::VolumeStep(step) => {
                self.adjust_volume(step);
                self.sync_status_view();
            }
            AppInput::Screenshot => {
                info!("Taking screenshot");
                if let Err(err) = std::process::Command::new("flameshot").arg("gui").spawn() {
                    error!("Failed to launch flameshot: {}", err);
                }
            }
            AppInput::SharedMessageReceived(message) => {
                self.process_shared_message(message);
                self.sync_workspaces_view();
                self.sync_layout_view();
                self.sync_status_view();
            }
            AppInput::SystemUpdate => {
                self.system_monitor.update_if_needed();
                if let Some(snapshot) = self.system_monitor.get_snapshot() {
                    let total = snapshot.memory_available + snapshot.memory_used;
                    self.memory_usage = if total > 0 {
                        (snapshot.memory_used as f64 / total as f64).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    self.cpu_usage = (snapshot.cpu_average as f64 / 100.0).clamp(0.0, 1.0);
                }
                self.refresh_volume_state();
                self.sync_status_view();
            }
            AppInput::UpdateTime => {
                self.update_time_display();
                self.sync_status_view();
            }
        }
    }
}

impl AppModel {
    fn update_time_display(&mut self) {
        let now = Local::now();
        let format_str = if self.show_seconds {
            "%Y-%m-%d %H:%M:%S"
        } else {
            "%Y-%m-%d %H:%M"
        };
        self.current_time = now.format(format_str).to_string();
    }

    fn refresh_volume_state(&mut self) {
        let _ = self.audio_manager.update_if_needed();
        if let Some(device) = self.audio_manager.get_master_device() {
            self.current_volume = Some(device.volume.clamp(0, 100));
            self.current_muted = device.is_muted;
        } else {
            self.current_volume = None;
            self.current_muted = false;
        }
    }

    fn toggle_mute(&mut self) {
        let _ = self.audio_manager.update_if_needed();
        let master = self.audio_manager.get_master_device().cloned();
        if let Some(device) = master {
            let volume = device.volume.clamp(0, 100);
            match self.audio_manager.toggle_mute(&device.name) {
                Ok(muted) => {
                    self.current_volume = Some(volume);
                    self.current_muted = muted;
                }
                Err(err) => warn!("toggle_mute failed: {:?}", err),
            }
        }
    }

    fn adjust_volume(&mut self, step: i32) {
        let _ = self.audio_manager.update_if_needed();
        let master = self.audio_manager.get_master_device().cloned();
        if let Some(device) = master {
            let previous_muted = device.is_muted;
            match self.audio_manager.adjust_volume(&device.name, step) {
                Ok(new_volume) => {
                    self.current_volume = Some(new_volume.clamp(0, 100));
                    self.current_muted = self
                        .audio_manager
                        .find_device(&device.name)
                        .map(|updated| updated.is_muted)
                        .unwrap_or(previous_muted);
                }
                Err(err) => warn!("adjust_volume failed: {:?}", err),
            }
        }
    }

    fn send_tag_command(&self, is_view: bool) {
        if self.active_tab >= 32 {
            return;
        }

        if let Some(shared_buffer) = &self.shared_buffer_opt {
            let tag_bit = 1u32 << self.active_tab;
            let monitor_id = self.monitor_num as i32;
            let command = if is_view {
                SharedCommand::view_tag(tag_bit, monitor_id)
            } else {
                SharedCommand::toggle_tag(tag_bit, monitor_id)
            };
            if let Err(err) = shared_buffer.send_command(command) {
                error!("Failed to send tag command: {}", err);
            }
        }
    }

    fn send_layout_command(&self, layout_index: u32) {
        if let Some(shared_buffer) = &self.shared_buffer_opt {
            let command =
                SharedCommand::new(CommandType::SetLayout, layout_index, self.monitor_num as i32);
            if let Err(err) = shared_buffer.send_command(command) {
                error!("Failed to send layout command: {}", err);
            }
        }
    }

    fn process_shared_message(&mut self, message: SharedMessage) {
        self.layout_symbol = message.monitor_info.get_ltsymbol();
        self.monitor_num = message.monitor_info.monitor_num as u8;
        self.tag_status_vec = message.monitor_info.tag_status_vec.to_vec();

        for (index, tag_status) in message.monitor_info.tag_status_vec.iter().enumerate() {
            if tag_status.is_selected {
                self.active_tab = index;
                break;
            }
        }
    }

    fn sync_all_views(&self) {
        self.sync_root_theme();
        self.sync_workspaces_view();
        self.sync_layout_view();
        self.sync_status_view();
    }

    fn sync_root_theme(&self) {
        self.root_window.remove_css_class("theme-dark");
        self.root_window.remove_css_class("theme-light");

        if self.theme_dark {
            self.root_window.add_css_class("theme-dark");
        } else {
            self.root_window.add_css_class("theme-light");
        }
    }

    fn sync_workspaces_view(&self) {
        self.workspaces.emit(WorkspacesInput::Sync(WorkspacesState {
            active_tab: self.active_tab,
            tag_status_vec: self.tag_status_vec.clone(),
        }));
    }

    fn sync_layout_view(&self) {
        self.layout_selector
            .emit(LayoutSelectorInput::Sync(LayoutSelectorState {
                layout_symbol: self.layout_symbol.clone(),
                layout_open: self.layout_open,
            }));
    }

    fn sync_status_view(&self) {
        self.status_strip.emit(StatusStripInput::Sync(StatusStripState {
            current_time: self.current_time.clone(),
            current_volume: self.current_volume,
            current_muted: self.current_muted,
            cpu_usage: self.cpu_usage,
            memory_usage: self.memory_usage,
            theme_dark: self.theme_dark,
            monitor_num: self.monitor_num,
        }));
    }
}

fn spawn_background_tasks(
    sender: ComponentSender<AppModel>,
    shared_buffer: Option<Arc<SharedRingBuffer>>,
) {
    let system_sender = sender.clone();
    glib::timeout_add_seconds_local(2, move || {
        system_sender.input(AppInput::SystemUpdate);
        ControlFlow::Continue
    });

    let time_sender = sender.clone();
    glib::timeout_add_seconds_local(1, move || {
        time_sender.input(AppInput::UpdateTime);
        ControlFlow::Continue
    });

    if let Some(shared_buffer) = shared_buffer {
        let shared_sender = sender.clone();
        thread::spawn(move || {
            shared_memory_worker(shared_buffer, shared_sender);
        });
    } else {
        warn!("No shared buffer, shared memory worker not started");
    }
}

fn shared_memory_worker(shared_buffer: Arc<SharedRingBuffer>, sender: ComponentSender<AppModel>) {
    let mut previous_timestamp: u128 = 0;
    loop {
        match shared_buffer.wait_for_message(Some(Duration::from_millis(2000))) {
            Ok(true) => {
                if let Ok(Some(message)) = shared_buffer.try_read_latest_message() {
                    let timestamp: u128 = message.timestamp.into();
                    if timestamp != previous_timestamp {
                        previous_timestamp = timestamp;
                        sender.input(AppInput::SharedMessageReceived(message));
                    }
                }
            }
            Ok(false) => {}
            Err(err) => {
                error!("[worker] wait_for_message failed: {}", err);
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
}
