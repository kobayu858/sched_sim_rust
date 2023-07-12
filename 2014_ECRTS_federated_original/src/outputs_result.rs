use lib::{
    graph_extension::{GraphExtension, NodeData},
    log::{DAGInfo, DAGSetInfo, ProcessorInfo},
    output_log::append_info_to_yaml,
    processor::ProcessorBase,
};
use petgraph::Graph;
use serde_derive::{Deserialize, Serialize};

use crate::federated::FederateResult;

#[derive(Serialize, Deserialize)]
struct ResultInfo<FederateResult> {
    result: FederateResult,
}

pub fn dump_processor_info_to_yaml(file_path: &str, processor: &impl ProcessorBase) {
    let number_of_cores = processor.get_number_of_cores();
    let processor_info = ProcessorInfo { number_of_cores };
    let yaml =
        serde_yaml::to_string(&processor_info).expect("Failed to serialize ProcessorInfo to YAML");
    append_info_to_yaml(file_path, &yaml);
}

pub fn dump_federated_result_to_file(file_path: &str, result: FederateResult) {
    let result_info = ResultInfo { result };
    let yaml =
        serde_yaml::to_string(&result_info).expect("Failed to serialize federated result to YAML");

    append_info_to_yaml(file_path, &yaml);
}

pub fn dump_dag_set_info_to_yaml(file_path: &str, mut dag_set: Vec<Graph<NodeData, i32>>) {
    let mut total_utilization = 0.0;
    let mut each_dag_info = Vec::new();

    for dag in dag_set.iter_mut() {
        let volume = dag.get_volume();
        let period = dag.get_head_period().unwrap();
        let critical_path = dag.get_critical_path();
        let critical_path_length = dag.get_total_wcet_from_nodes(&critical_path);
        total_utilization += volume as f32 / period as f32;

        let dag_info = DAGInfo {
            critical_path_length,
            end_to_end_deadline: period,
            volume,
        };

        each_dag_info.push(dag_info);
    }

    let dag_set_info = DAGSetInfo {
        total_utilization,
        each_dag_info,
    };

    let yaml =
        serde_yaml::to_string(&dag_set_info).expect("Failed to serialize DAGSetInfo to YAML");

    append_info_to_yaml(file_path, &yaml);
}

#[cfg(test)]
mod tests {
    use super::*;
    use lib::{graph_extension::NodeData, homogeneous, output_log::create_scheduler_log_yaml_file};
    use petgraph::Graph;
    use std::{collections::HashMap, fs::remove_file};

    fn create_node(id: i32, key: &str, value: i32) -> NodeData {
        let mut params = HashMap::new();
        params.insert(key.to_string(), value);
        NodeData { id, params }
    }

    fn create_high_utilization_dag() -> Graph<NodeData, i32> {
        let mut dag = Graph::<NodeData, i32>::new();
        let n0 = {
            let mut params = HashMap::new();
            params.insert("execution_time".to_owned(), 4);
            params.insert("period".to_owned(), 10);
            dag.add_node(NodeData { id: 3, params })
        };
        let n1 = dag.add_node(create_node(1, "execution_time", 4));
        let n2 = dag.add_node(create_node(2, "execution_time", 3));
        let n3 = dag.add_node(create_node(3, "execution_time", 3));
        dag.add_edge(n0, n1, 1);
        dag.add_edge(n0, n2, 1);
        dag.add_edge(n0, n3, 1);

        dag
    }

    fn create_low_utilization_dag() -> Graph<NodeData, i32> {
        let mut dag = Graph::<NodeData, i32>::new();
        let n0 = {
            let mut params = HashMap::new();
            params.insert("execution_time".to_owned(), 3);
            params.insert("period".to_owned(), 30);
            dag.add_node(NodeData { id: 2, params })
        };
        let n1 = dag.add_node(create_node(0, "execution_time", 3));
        let n2 = dag.add_node(create_node(1, "execution_time", 4));

        dag.add_edge(n0, n1, 1);
        dag.add_edge(n0, n2, 1);
        dag
    }

    fn create_period_exceeding_dag() -> Graph<NodeData, i32> {
        let mut dag = Graph::<NodeData, i32>::new();
        let mut params = HashMap::new();
        params.insert("execution_time".to_owned(), 20);
        params.insert("period".to_owned(), 10);
        dag.add_node(NodeData { id: 0, params });
        dag
    }

    #[test]
    fn test_dump_federated_result_to_file_normal() {
        let number_of_cores = 40;
        let mut dag_set = vec![
            create_high_utilization_dag(),
            create_high_utilization_dag(),
            create_low_utilization_dag(),
        ];
        let result = crate::federated::federated(&mut dag_set, number_of_cores);
        let file_path =
            create_scheduler_log_yaml_file("../lib/tests", "test_dump_federated_info_normal");
        dump_federated_result_to_file(&file_path, result);

        let file_contents = std::fs::read_to_string(&file_path).unwrap();
        let result_info: ResultInfo<FederateResult> = serde_yaml::from_str(&file_contents).unwrap();

        assert_eq!(
            result_info.result,
            FederateResult::Schedulable {
                high_dedicated_cores: 6,
                low_dedicated_cores: 34,
            }
        );

        remove_file(file_path).unwrap();
    }

    #[test]
    fn test_dump_federated_result_to_file_lack_cores_for_high_tasks() {
        let number_of_cores = 1;
        let mut dag_set = vec![
            create_high_utilization_dag(),
            create_high_utilization_dag(),
            create_low_utilization_dag(),
        ];
        let result = crate::federated::federated(&mut dag_set, number_of_cores);
        let file_path = create_scheduler_log_yaml_file(
            "../lib/tests",
            "test_federated_lack_cores_for_high_tasks",
        );
        dump_federated_result_to_file(&file_path, result);

        let file_contents = std::fs::read_to_string(&file_path).unwrap();
        let result_info: ResultInfo<FederateResult> = serde_yaml::from_str(&file_contents).unwrap();

        assert_eq!(
            result_info.result,
            FederateResult::Unschedulable {
                reason: (String::from("Insufficient number of cores for high-utilization tasks.")),
                insufficient_cores: 2
            }
        );

        remove_file(file_path).unwrap();
    }

    #[test]
    fn test_dump_federated_result_to_file_lack_cores_for_low_tasks() {
        let number_of_cores = 3;
        let mut dag_set = vec![
            create_high_utilization_dag(),
            create_low_utilization_dag(),
            create_low_utilization_dag(),
        ];
        let result = crate::federated::federated(&mut dag_set, number_of_cores);
        let file_path = create_scheduler_log_yaml_file(
            "../lib/tests",
            "test_federated_lack_cores_for_low_tasks",
        );
        dump_federated_result_to_file(&file_path, result);

        let file_contents = std::fs::read_to_string(&file_path).unwrap();
        let result_info: ResultInfo<FederateResult> = serde_yaml::from_str(&file_contents).unwrap();

        assert_eq!(
            result_info.result,
            FederateResult::Unschedulable {
                reason: (String::from("Insufficient number of cores for low-utilization tasks.")),
                insufficient_cores: 2
            }
        );

        remove_file(file_path).unwrap();
    }

    #[test]
    fn test_dump_federated_result_to_file_unsuited_tasks() {
        let number_of_cores = 1;
        let mut dag_set = vec![create_period_exceeding_dag()];
        let result = crate::federated::federated(&mut dag_set, number_of_cores);
        let file_path =
            create_scheduler_log_yaml_file("../lib/tests", "test_federated_unsuited_tasks");
        dump_federated_result_to_file(&file_path, result);

        let file_contents = std::fs::read_to_string(&file_path).unwrap();
        let result_info: ResultInfo<FederateResult> = serde_yaml::from_str(&file_contents).unwrap();

        assert_eq!(
            result_info.result,
            FederateResult::Unschedulable {
                reason: (String::from(
                    "The critical path length is greater than end_to_end_deadline."
                )),
                insufficient_cores: 0
            }
        );

        remove_file(file_path).unwrap();
    }

    #[test]
    fn test_dump_dag_set_info_to_yaml_file_normal() {
        let dag_set = vec![create_high_utilization_dag(), create_high_utilization_dag()];
        let file_path = create_scheduler_log_yaml_file("../lib/tests", "dag_set_info");
        dump_dag_set_info_to_yaml(&file_path, dag_set);

        let file_contents = std::fs::read_to_string(&file_path).unwrap();
        let dag_set: DAGSetInfo = serde_yaml::from_str(&file_contents).unwrap();

        assert_eq!(dag_set.total_utilization, 2.8);
        assert_eq!(dag_set.each_dag_info.len(), 2);
        assert_eq!(dag_set.each_dag_info[1].critical_path_length, 8);
        assert_eq!(dag_set.each_dag_info[1].end_to_end_deadline, 10);
        assert_eq!(dag_set.each_dag_info[1].volume, 14);

        remove_file(file_path).unwrap();
    }

    #[test]
    fn test_dump_processor_info_to_yaml() {
        let file_path = create_scheduler_log_yaml_file("../lib/tests", "processor_info");
        let homogeneous_processor = homogeneous::HomogeneousProcessor::new(4);
        dump_processor_info_to_yaml(&file_path, &homogeneous_processor);

        let file_contents = std::fs::read_to_string(&file_path).unwrap();
        let number_of_cores: ProcessorInfo = serde_yaml::from_str(&file_contents).unwrap();

        assert_eq!(number_of_cores.number_of_cores, 4);

        remove_file(file_path).unwrap();
    }
}
