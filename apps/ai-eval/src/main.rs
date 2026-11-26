#[tokio::main]
async fn main() -> std::process::ExitCode {
    warptalk_ai_eval::cli::main(std::env::args().skip(1).collect()).await
}
