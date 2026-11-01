use std::io::Write;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use gp_tray::engine::Engine;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["status"] => print_state().await,
        ["status", "--watch"] => watch_state().await,
        ["--version"] | ["-V"] => {
            println!("gp-tray {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        ["--help"] | ["-h"] => {
            print_usage();
            ExitCode::SUCCESS
        }
        [] => {
            eprintln!("gp-tray: the tray UI is not wired up yet");
            ExitCode::FAILURE
        }
        _ => {
            print_usage();
            ExitCode::from(2)
        }
    }
}

fn print_usage() {
    println!(
        "gp-tray {}\n\
         \n\
         Usage:\n  \
         gp-tray                 run the tray\n  \
         gp-tray status          print the current VPN state\n  \
         gp-tray status --watch  print every state transition until interrupted\n",
        env!("CARGO_PKG_VERSION")
    );
}

async fn print_state() -> ExitCode {
    let engine = Engine::spawn().await;
    println!("{}", engine.state());
    ExitCode::SUCCESS
}

async fn watch_state() -> ExitCode {
    let engine = Engine::spawn().await;
    let mut states = engine.state_stream();
    print_transition(*states.borrow_and_update());

    loop {
        tokio::select! {
            changed = states.changed() => {
                if changed.is_err() {
                    break;
                }
                print_transition(*states.borrow_and_update());
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    ExitCode::SUCCESS
}

fn print_transition(state: impl std::fmt::Display) {
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{}  {state}", format_utc(SystemTime::now()));
    let _ = stdout.flush();
}

fn format_utc(time: SystemTime) -> String {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = since_epoch.as_secs();
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let seconds_of_day = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        seconds_of_day / 3_600,
        seconds_of_day % 3_600 / 60,
        seconds_of_day % 60,
        since_epoch.subsec_millis()
    )
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let days = days_since_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = (days - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_position + 2) / 5 + 1) as u32;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formats_utc_timestamps() {
        let cases = [
            (UNIX_EPOCH, "1970-01-01T00:00:00.000Z"),
            (
                UNIX_EPOCH + Duration::from_millis(500),
                "1970-01-01T00:00:00.500Z",
            ),
            (
                UNIX_EPOCH + Duration::from_secs(1_700_000_000),
                "2023-11-14T22:13:20.000Z",
            ),
            (
                UNIX_EPOCH + Duration::from_secs(1_709_164_800),
                "2024-02-29T00:00:00.000Z",
            ),
            (
                UNIX_EPOCH + Duration::from_secs(951_782_400),
                "2000-02-29T00:00:00.000Z",
            ),
        ];
        for (time, expected) in cases {
            assert_eq!(format_utc(time), expected);
        }
    }
}
