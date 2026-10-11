mod auth;
mod cluster;
mod config;
mod db;
mod error;
mod leader;
mod namespace;
mod path;
mod placement;
mod read_plan;
mod pb;
mod time;
mod upload;
#[cfg(test)]
mod tests;

use clap::Parser;

use config::Config;

fn main() {
    dotenvy::dotenv().ok();
    let config = Config::parse();
    println!("master {} starting on port {}", config.id, config.port);
}
