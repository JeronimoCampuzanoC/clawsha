mod config;
mod error;
mod path;
mod pb;
#[cfg(test)]
mod tests;

use clap::Parser;

use config::Config;

fn main() {
    dotenvy::dotenv().ok();
    let config = Config::parse();
    println!("master {} starting on port {}", config.id, config.port);
}
