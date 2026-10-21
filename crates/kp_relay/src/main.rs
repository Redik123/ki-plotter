//! `ki-plotter-relay`: the service on the Raspberry Pi wired to the cutter.

use std::process::ExitCode;

use kp_relay::Config;

const HELP: &str = "\
ki-plotter-relay — relais réseau entre KI Plotter et le plotter

Usage : ki-plotter-relay [--config FICHIER] [--name NOM] [--listen ADRESSE:PORT]
                         [--device auto|/dev/ttyUSB0|/dev/usb/lp0] [--baud N] [--flow none|hardware|software]
        ki-plotter-relay --list-ports

Les options de la ligne de commande passent avant celles du fichier.
Page d'état : http://<adresse du Pi>:7870/";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("ki-plotter-relay {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--list-ports") {
        for p in kp_link::list_ports() {
            println!(
                "{}\t{:?}\t{}",
                p.name,
                p.kind,
                p.description.unwrap_or_default()
            );
        }
        return ExitCode::SUCCESS;
    }
    match parse(&args) {
        Ok(config) => {
            eprintln!(
                "KI Plotter relais {} « {} » sur {} — plotter : {}",
                env!("CARGO_PKG_VERSION"),
                config.name,
                config.listen,
                config.device
            );
            if let Err(e) = kp_relay::run(config) {
                eprintln!("Impossible de démarrer : {e}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

fn parse(args: &[String]) -> Result<Config, String> {
    let mut config = Config::default();
    // The file first, so the command line can override it.
    if let Some(i) = args.iter().position(|a| a == "--config") {
        let path = args.get(i + 1).ok_or("--config : fichier manquant")?;
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("Impossible de lire « {path} » : {e}"))?;
        config
            .load_file(&text)
            .map_err(|e| format!("{path} : {e}"))?;
    }
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let key = match a.as_str() {
            "--config" => {
                it.next();
                continue;
            }
            "--name" => "name",
            "--listen" => "listen",
            "--device" => "device",
            "--baud" => "baud",
            "--flow" => "flow",
            other => return Err(format!("Option inconnue : {other}")),
        };
        let v = it.next().ok_or_else(|| format!("{a} : valeur manquante"))?;
        config.set(key, v)?;
    }
    Ok(config)
}
