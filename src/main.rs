use std::env;
use xbar_core::initialize_logging;
mod app;
mod components;

use log::info;
use relm4::RelmApp;

use crate::app::AppModel;

fn main() {
    let args: Vec<String> = env::args().collect();
    let shared_path = args.iter().skip(1).last().cloned().unwrap_or_default();

    if let Err(err) = initialize_logging("relm_bar", &shared_path) {
        eprintln!("Failed to initialize logging: {}", err);
        std::process::exit(1);
    }

    let monitor_id = env::var("JWM_MONITOR_ID").unwrap_or_else(|_| {
        shared_path
            .split('_')
            .last()
            .and_then(|segment| segment.parse::<i32>().ok())
            .map(|id| id.to_string())
            .unwrap_or_else(|| "0".to_string())
    });

    let app_id = format!("dev.relm.bar.mon{}", monitor_id);
    info!("Starting relm_bar with gtk_bar-aligned UI and relm4 component flow");
    info!("Application ID: {}", app_id);

    RelmApp::new(&app_id).run::<AppModel>(shared_path);
}