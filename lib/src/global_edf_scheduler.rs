use petgraph::graph::{Graph, NodeIndex};
use std::cmp::Ordering;
use std::collections::BTreeSet;

use crate::core::ProcessResult;
use crate::scheduler::DAGStateManager;
use crate::{
    graph_extension::{GraphExtension, NodeData},
    homogeneous::HomogeneousProcessor,
    log::DAGSetSchedulerLog,
    processor::ProcessorBase,
    scheduler::DAGSetSchedulerBase,
    util::get_hyper_period,
};

// Define a new wrapper type
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeDataWrapper(NodeData);

impl NodeDataWrapper {
    fn convert_node_data(&self) -> NodeData {
        self.0.clone()
    }
}

impl PartialOrd for NodeDataWrapper {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        // Define the keys to compare
        let key1 = "period";
        let key2 = "dag_id";
        match (self.0.params.get(key1), other.0.params.get(key1)) {
            (Some(self_val), Some(other_val)) => match self_val.cmp(other_val) {
                // If the keys are equal, compare by id
                Ordering::Equal => match self.0.id.partial_cmp(&other.0.id) {
                    // If the ids are also equal, compare by dag_id
                    Some(Ordering::Equal) => {
                        match (self.0.params.get(key2), other.0.params.get(key2)) {
                            (Some(self_dag), Some(other_dag)) => Some(self_dag.cmp(other_dag)),
                            (None, None) => Some(Ordering::Equal),
                            (Some(_), None) => Some(Ordering::Greater),
                            (None, Some(_)) => Some(Ordering::Less),
                        }
                    }
                    other => other,
                },
                other => Some(other),
            },
            // If neither of the keys exists, compare by id
            (None, None) => match self.0.id.partial_cmp(&other.0.id) {
                // If the ids are equal, compare by dag_id
                Some(Ordering::Equal) => {
                    match (self.0.params.get(key2), other.0.params.get(key2)) {
                        (Some(self_dag), Some(other_dag)) => Some(self_dag.cmp(other_dag)),
                        (None, None) => Some(Ordering::Equal),
                        (Some(_), None) => Some(Ordering::Greater),
                        (None, Some(_)) => Some(Ordering::Less),
                    }
                }
                other => other,
            },
            // If only one of the keys exists, the one with the key is greater
            (Some(_), None) => Some(Ordering::Greater),
            (None, Some(_)) => Some(Ordering::Less),
        }
    }
}

impl Ord for NodeDataWrapper {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap_or(Ordering::Equal)
    }
}

pub struct GlobalEDFScheduler {
    dag_set: Vec<Graph<NodeData, i32>>,
    processor: HomogeneousProcessor,
    current_time: i32,
    log: DAGSetSchedulerLog,
    managers: Vec<DAGStateManager>,
    ready_queue: BTreeSet<NodeDataWrapper>,
}

impl DAGSetSchedulerBase<HomogeneousProcessor> for GlobalEDFScheduler {
    fn new(dag_set: &[Graph<NodeData, i32>], processor: &HomogeneousProcessor) -> Self {
        Self {
            dag_set: dag_set.to_vec(),
            processor: processor.clone(),
            current_time: 0,
            log: DAGSetSchedulerLog::new(dag_set, processor.get_number_of_cores()),
            managers: vec![DAGStateManager::new_basic(); dag_set.len()],
            ready_queue: BTreeSet::new(),
        }
    }

    fn initialize(&mut self) {
        // Initialize DAG
        for (dag_id, dag) in self.dag_set.iter_mut().enumerate() {
            dag.set_dag_id(dag_id);
            dag.set_dag_period(dag.get_head_period().unwrap());
        }
    }

    fn release_dag(&mut self, log: &mut DAGSetSchedulerLog) {
        for dag in self.dag_set.iter_mut() {
            let dag_id = dag.get_dag_id();
            if self.current_time
                == dag.get_head_offset()
                    + dag.get_head_period().unwrap() * self.managers[dag_id].get_release_count()
            {
                self.managers[dag_id].release();
                self.managers[dag_id].increment_release_count();
                log.write_dag_release_time(dag_id, self.current_time);
            }
        }
    }

    fn start_dag(&mut self, log: &mut DAGSetSchedulerLog) {
        let mut idle_core_num = self.processor.get_idle_core_num();
        for (dag_id, manager) in self.managers.iter_mut().enumerate() {
            if idle_core_num > 0 && !manager.get_is_started() && manager.get_is_released() {
                manager.start();
                idle_core_num -= 1;
                // Add the source node to the ready queue
                let dag = &self.dag_set[dag_id];
                let source_node = &dag[dag.get_source_nodes()[0]];
                self.ready_queue
                    .insert(NodeDataWrapper(source_node.clone()));
                log.write_dag_start_time(dag_id, self.current_time);
            };
        }
    }

    fn allocate_node(&mut self, log: &mut DAGSetSchedulerLog) {
        while !self.ready_queue.is_empty() {
            match self.processor.get_idle_core_index() {
                Some(idle_core_index) => {
                    let ready_node_data = self.ready_queue.pop_first().unwrap().convert_node_data();
                    self.processor
                        .allocate_specific_core(idle_core_index, &ready_node_data);
                    log.write_allocating_node(
                        ready_node_data.get_params_value("dag_id") as usize,
                        ready_node_data.get_id() as usize,
                        idle_core_index,
                        self.current_time,
                        ready_node_data.get_params_value("execution_time"),
                    );
                }
                None => break,
            };
        }
    }

    fn process_unit_time(&mut self) -> Vec<ProcessResult> {
        self.current_time += 1;
        self.processor.process()
    }

    fn handling_nodes_finished(
        &mut self,
        log: &mut DAGSetSchedulerLog,
        process_result: &[ProcessResult],
    ) {
        for result in process_result.clone() {
            if let ProcessResult::Done(node_data) = result {
                log.write_finishing_node(node_data, self.current_time);

                // Increase pre_done_count of successor nodes
                let dag_id = node_data.get_params_value("dag_id") as usize;
                let dag = &mut self.dag_set[dag_id];
                let suc_nodes = dag
                    .get_suc_nodes(NodeIndex::new(node_data.get_id() as usize))
                    .unwrap_or_default();
                if suc_nodes.is_empty() {
                    log.write_dag_finish_time(dag_id, self.current_time);
                    // Reset the state of the DAG
                    dag.reset_pre_done_count();
                    self.managers[dag_id].reset_state();
                } else {
                    for suc_node in suc_nodes {
                        dag.increment_pre_done_count(suc_node);
                    }
                }
            }
        }
    }

    fn insert_ready_node(&mut self, process_result: &[ProcessResult]) {
        for result in process_result {
            if let ProcessResult::Done(node_data) = result {
                let dag_id = node_data.get_params_value("dag_id") as usize;
                let dag = &mut self.dag_set[dag_id];
                let suc_nodes = dag
                    .get_suc_nodes(NodeIndex::new(node_data.get_id() as usize))
                    .unwrap_or_default();

                // If all preceding nodes have finished, add the node to the ready queue
                for suc_node in suc_nodes {
                    if dag.is_node_ready(suc_node) {
                        self.ready_queue
                            .insert(NodeDataWrapper(dag[suc_node].clone()));
                    }
                }
            }
        }
    }

    fn schedule(&mut self) -> i32 {
        // Initialize DAGStateManagers
        // let mut managers = vec![DAGStateManager::new(); self.dag_set.len()];

        self.initialize();

        // Start scheduling
        let mut log = self.get_log();
        let hyper_period = get_hyper_period(&self.dag_set);
        while self.current_time < hyper_period {
            // release DAGs
            self.release_dag(&mut log);
            // Start DAGs if there are free cores
            self.start_dag(&mut log);
            // Allocate the nodes of ready_queue to idle cores
            self.allocate_node(&mut log);
            // Process unit time
            let process_result: Vec<ProcessResult> = self.process_unit_time();
            // Post-process on completion of node execution
            self.handling_nodes_finished(&mut log, &process_result);
            // Add the node to the ready queue when all preceding nodes have finished
            self.insert_ready_node(&process_result);
        }

        log.calculate_utilization(self.current_time);
        self.set_log(log);

        self.current_time
    }

    fn get_log(&self) -> DAGSetSchedulerLog {
        self.log.clone()
    }

    fn set_log(&mut self, log: DAGSetSchedulerLog) {
        self.log = log;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::load_yaml;
    use std::{collections::BTreeMap, fs::remove_file};

    fn create_node(id: i32, key: &str, value: i32) -> NodeData {
        let mut params = BTreeMap::new();
        params.insert(key.to_string(), value);
        NodeData { id, params }
    }

    fn create_sample_dag() -> Graph<NodeData, i32> {
        let mut dag = Graph::<NodeData, i32>::new();
        // cX is the Xth critical node.
        let c0 = dag.add_node(create_node(0, "execution_time", 10));
        let c1 = dag.add_node(create_node(1, "execution_time", 20));
        let c2 = dag.add_node(create_node(2, "execution_time", 20));
        dag.add_param(c0, "period", 150);
        dag.add_param(c2, "end_to_end_deadline", 50);
        // nY_X is the Yth suc node of cX.
        let n0_0 = dag.add_node(create_node(3, "execution_time", 10));
        let n1_0 = dag.add_node(create_node(4, "execution_time", 10));

        // Create critical path edges
        dag.add_edge(c0, c1, 1);
        dag.add_edge(c1, c2, 1);

        // Create non-critical path edges
        dag.add_edge(c0, n0_0, 1);
        dag.add_edge(c0, n1_0, 1);
        dag.add_edge(n0_0, c2, 1);
        dag.add_edge(n1_0, c2, 1);

        dag
    }

    fn create_sample_dag2() -> Graph<NodeData, i32> {
        let mut dag = Graph::<NodeData, i32>::new();
        // cX is the Xth critical node.
        let c0 = dag.add_node(create_node(0, "execution_time", 10));
        let c1 = dag.add_node(create_node(1, "execution_time", 20));
        let c2 = dag.add_node(create_node(2, "execution_time", 20));
        dag.add_param(c0, "period", 100);
        dag.add_param(c2, "end_to_end_deadline", 60);
        // nY_X is the Yth suc node of cX.
        let n0_0 = dag.add_node(create_node(3, "execution_time", 10));

        // Create critical path edges
        dag.add_edge(c0, c1, 1);
        dag.add_edge(c1, c2, 1);

        // Create non-critical path edges
        dag.add_edge(c0, n0_0, 1);
        dag.add_edge(n0_0, c2, 1);

        dag
    }

    #[test]
    fn test_global_edf_normal() {
        let dag = create_sample_dag();
        let dag2 = create_sample_dag2();
        let dag_set = vec![dag, dag2];

        let processor = HomogeneousProcessor::new(4);

        let mut global_edf_scheduler = GlobalEDFScheduler::new(&dag_set, &processor);
        let time = global_edf_scheduler.schedule();

        assert_eq!(time, 300);

        let file_path = global_edf_scheduler.dump_log("../lib/tests", "edf_test");
        let yaml_docs = load_yaml(&file_path);
        let yaml_doc = &yaml_docs[0];

        // Check the value of total_utilization
        assert_eq!(
            yaml_doc["dag_set_info"]["total_utilization"]
                .as_f64()
                .unwrap(),
            3.8095236
        );

        // Check the value of each_dag_info
        let each_dag_info = &yaml_doc["dag_set_info"]["each_dag_info"][0];
        assert_eq!(each_dag_info["critical_path_length"].as_i64().unwrap(), 50);
        assert_eq!(each_dag_info["period"].as_i64().unwrap(), 150);
        assert_eq!(each_dag_info["end_to_end_deadline"].as_i64().unwrap(), 50);
        assert_eq!(each_dag_info["volume"].as_i64().unwrap(), 70);
        assert_eq!(each_dag_info["utilization"].as_f64().unwrap(), 2.142857);

        // Check the value of processor_info
        assert_eq!(
            yaml_doc["processor_info"]["number_of_cores"]
                .as_i64()
                .unwrap(),
            4
        );

        // Check the value of dag_set_log
        let dag_set_log = &yaml_doc["dag_set_log"][0];
        assert_eq!(dag_set_log["dag_id"].as_i64().unwrap(), 0);
        let release_time = &dag_set_log["release_time"];
        assert_eq!(release_time[0].as_i64().unwrap(), 0);
        assert_eq!(release_time[1].as_i64().unwrap(), 150);
        let start_time = &dag_set_log["start_time"];
        assert_eq!(start_time[0].as_i64().unwrap(), 0);
        assert_eq!(start_time[1].as_i64().unwrap(), 150);
        let finish_time = &dag_set_log["finish_time"];
        assert_eq!(finish_time[0].as_i64().unwrap(), 50);
        assert_eq!(finish_time[1].as_i64().unwrap(), 200);

        // Check the value of node_set_logs
        let node_set_logs = &yaml_doc["node_set_logs"][0][0];
        let core_id = &node_set_logs["core_id"];
        assert_eq!(core_id[0].as_i64().unwrap(), 1);
        assert_eq!(core_id[1].as_i64().unwrap(), 0);
        assert_eq!(node_set_logs["dag_id"].as_i64().unwrap(), 0);
        assert_eq!(node_set_logs["node_id"].as_i64().unwrap(), 0);
        let start_time = &node_set_logs["start_time"];
        assert_eq!(start_time[0].as_i64().unwrap(), 0);
        assert_eq!(start_time[1].as_i64().unwrap(), 150);
        let finish_time = &node_set_logs["finish_time"];
        assert_eq!(finish_time[0].as_i64().unwrap(), 10);
        assert_eq!(finish_time[1].as_i64().unwrap(), 160);

        // Check the value of processor_log
        let processor_log = &yaml_doc["processor_log"];
        assert_eq!(
            processor_log["average_utilization"].as_f64().unwrap(),
            0.26666668
        );
        assert_eq!(
            processor_log["variance_utilization"].as_f64().unwrap(),
            0.06055556
        );

        // Check the value of core_logs
        let core_logs = &processor_log["core_logs"][0];
        assert_eq!(core_logs["core_id"].as_i64().unwrap(), 0);
        assert_eq!(core_logs["total_proc_time"].as_i64().unwrap(), 200);
        assert_eq!(core_logs["utilization"].as_f64().unwrap(), 0.6666667);

        remove_file(file_path).unwrap();
    }
}