//! `peet`: PeetCAD's command line. Everything it does is in the library; this only hands
//! over the arguments and the standard streams.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = peet_cli::run(
        &args,
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    std::process::exit(code);
}
