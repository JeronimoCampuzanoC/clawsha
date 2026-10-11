mod config;
mod pb;

use clap::Parser;

use config::Config;

fn main() {
    dotenvy::dotenv().ok();
    let config = Config::parse();
    println!("master {} starting on port {}", config.id, config.port);
}
