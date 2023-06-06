use clap::Parser;
mod parallel_provider_consumer;
mod prioritization_cpc_model;

use lib::dag_creator::*;
use lib::fixed_priority_scheduler::fixed_priority_scheduler;
use lib::homogeneous;
use lib::processor::ProcessorBase;

/// Application description and arguments definition using clap crate
#[derive(Parser)]
#[clap(
    name = "sched_sim",
    author = "Yutaro kobayashi",
    version = "v1.0.0",
    about = "Application short description."
)]

/// Application arguments definition using clap crate
struct AppArg {
    #[clap(short = 'f', long = "dag_file_path", required = true)]
    dag_file_path: String,
    #[clap(short = 'c', long = "number_of_cores", required = true)]
    number_of_cores: usize,
    #[clap(short = 'o', long = "output_dir_path", default_value = "../outputs")]
    output_dir_path: String,
}
fn main() {
    let arg: AppArg = AppArg::parse();
    let mut dag = create_dag_from_yaml(&arg.dag_file_path);
    let number_of_cores = arg.number_of_cores;
    prioritization_cpc_model::assign_priority_to_cpc_model(&mut dag);
    let _sched_result = fixed_priority_scheduler(
        &mut homogeneous::HomogeneousProcessor::new(number_of_cores),
        &mut dag,
    );
}
