use std::collections::VecDeque;

use crate::{
    core::ProcessResult,
    graph_extension::{GraphExtension, NodeData},
    log::*,
    processor::ProcessorBase,
    util::create_yaml,
};
use chrono::{DateTime, Utc};
use petgraph::graph::{Graph, NodeIndex};

const DUMMY_EXECUTION_TIME: i32 = 1;

pub fn create_scheduler_log_yaml(dir_path: &str, alg_name: &str) -> String {
    let now: DateTime<Utc> = Utc::now();
    let date = now.format("%Y-%m-%d-%H-%M-%S").to_string();
    let file_name = format!("{}-{}-log", date, alg_name);
    create_yaml(dir_path, &file_name)
}

pub trait DAGSchedulerBase<T>
where
    T: ProcessorBase + Clone,
{
    fn new(dag: &Graph<NodeData, i32>, processor: &T) -> Self
    where
        Self: Sized;
    fn set_dag(&mut self, dag: &Graph<NodeData, i32>);
    fn set_processor(&mut self, processor: &T);
    fn set_log(&mut self, log: DAGSchedulerLog);
    fn get_dag(&self) -> Graph<NodeData, i32>;
    fn get_processor(&self) -> T;
    fn get_log(&self) -> DAGSchedulerLog;
    fn schedule(&mut self) -> (i32, VecDeque<NodeIndex>) {
        {
            let mut dag = self.get_dag(); //To avoid adding pre_node_count to the original DAG
            let mut processor = self.get_processor();
            let mut ready_queue = VecDeque::new();
            let mut log = self.get_log();
            let mut execution_order = VecDeque::new();
            let source_node_i = dag.add_dummy_source_node();

            dag[source_node_i]
                .params
                .insert("execution_time".to_string(), DUMMY_EXECUTION_TIME);
            let sink_node_i = dag.add_dummy_sink_node();
            dag[sink_node_i]
                .params
                .insert("execution_time".to_string(), DUMMY_EXECUTION_TIME);

            ready_queue.push_back(dag[source_node_i].clone());

            let mut current_time = 0;
            loop {
                Self::sort_ready_queue(&mut ready_queue);

                //Assign the highest priority task first to the first idle core found.
                while let Some(core_index) = processor.get_idle_core_index() {
                    if let Some(node_d) = ready_queue.pop_front() {
                        processor.allocate_specific_core(core_index, &node_d);

                        if node_d.id != dag[source_node_i].id && node_d.id != dag[sink_node_i].id {
                            log.write_allocating_node(
                                &node_d,
                                core_index,
                                current_time - DUMMY_EXECUTION_TIME,
                            );
                        }
                        execution_order.push_back(NodeIndex::new(node_d.id as usize));
                    } else {
                        break;
                    }
                }

                //Move one unit time so that the core state of the previous loop does not remain.
                let mut process_result = processor.process();
                current_time += 1;

                //Process until there is a task finished.
                while !process_result
                    .iter()
                    .any(|result| matches!(result, ProcessResult::Done(_)))
                {
                    process_result = processor.process();
                    current_time += 1;
                }

                let finish_nodes: Vec<NodeIndex> = process_result
                    .iter()
                    .filter_map(|result| {
                        if let ProcessResult::Done(node_data) = result {
                            let node_id = node_data.id as usize;
                            let node_i = NodeIndex::new(node_id);
                            if node_i != source_node_i && node_i != sink_node_i {
                                log.write_finishing_node(
                                    node_data,
                                    current_time - DUMMY_EXECUTION_TIME,
                                );
                            }
                            Some(node_i)
                        } else {
                            None
                        }
                    })
                    .collect();

                if finish_nodes.len() == 1 && dag.get_suc_nodes(finish_nodes[0]).is_none() {
                    break; // The scheduling has finished because the dummy sink node has completed.
                }

                //Executable if all predecessor nodes are done
                for finish_node in finish_nodes {
                    let suc_nodes = dag.get_suc_nodes(finish_node).unwrap_or_default();
                    for suc_node in suc_nodes {
                        dag.increment_pre_done_count(suc_node);
                        if dag.is_node_ready(suc_node) {
                            ready_queue.push_back(dag[suc_node].clone());
                        }
                    }
                }
            }

            //remove dummy nodes
            dag.remove_dummy_sink_node();
            dag.remove_dummy_source_node();

            //Remove the dummy node from the execution order.
            execution_order.pop_back();
            execution_order.pop_front();

            let schedule_length = current_time - DUMMY_EXECUTION_TIME * 2;
            log.calculate_utilization(schedule_length);

            self.set_log(log);

            //Return the normalized total time taken to finish all tasks.
            (schedule_length, execution_order)
        }
    }
    fn sort_ready_queue(ready_queue: &mut VecDeque<NodeData>);
    fn dump_log(&self, dir_path: &str, alg_name: &str) -> String {
        let file_path = create_scheduler_log_yaml(dir_path, alg_name);
        self.get_log().dump_log_to_yaml(&file_path);

        file_path
    }
}

#[derive(Clone, Debug)]
pub struct DAGStateManager {
    release_count: i32,
    is_started: bool,
    is_released: bool,
    num_using_cores: Option<i32>,
    num_allocated_cores: Option<i32>,
    minimum_cores: Option<i32>,
    execution_order: Option<VecDeque<NodeIndex>>,
    initial_execution_order: Option<VecDeque<NodeIndex>>,
}

impl DAGStateManager {
    pub fn new_basic() -> Self {
        Self {
            release_count: 0,
            is_started: false,
            is_released: false,
            num_using_cores: None,
            num_allocated_cores: None,
            minimum_cores: None,
            execution_order: None,
            initial_execution_order: None,
        }
    }

    pub fn new_expended() -> Self {
        Self {
            release_count: 0,
            is_started: false,
            is_released: false,
            num_using_cores: Some(0),
            num_allocated_cores: Some(0),
            minimum_cores: Some(0),
            execution_order: Some(VecDeque::new()),
            initial_execution_order: Some(VecDeque::new()),
        }
    }

    pub fn get_release_count(&self) -> i32 {
        self.release_count
    }

    pub fn increment_release_count(&mut self) {
        self.release_count += 1;
    }

    pub fn start(&mut self) {
        self.is_started = true;
        if self.num_allocated_cores.is_some() {
            self.num_allocated_cores = self.minimum_cores;
        }
    }

    pub fn get_is_started(&self) -> bool {
        self.is_started
    }

    pub fn can_start(&self, unused_cores: i32) -> bool {
        self.minimum_cores <= Some(unused_cores)
    }

    pub fn reset_state(&mut self) {
        self.is_started = false;
        self.is_released = false;
        if self.execution_order.is_some()
            && self.initial_execution_order.is_some()
            && self.num_allocated_cores.is_some()
        {
            self.set_execution_order(self.initial_execution_order.clone());
            self.free_allocated_cores(); //When the last node is finished, the core allocated to dag is released.
        }
    }

    pub fn release(&mut self) {
        self.is_released = true;
    }

    pub fn get_is_released(&self) -> bool {
        self.is_released
    }

    pub fn decrement_num_using_cores(&mut self) {
        match &mut self.num_using_cores {
            Some(cores) => *cores -= 1,
            None => panic!("num_using_cores is None!"),
        }
    }

    pub fn get_unused_cores(&self) -> i32 {
        let allocated_cores = self
            .num_allocated_cores
            .expect("num_allocated_cores is None!");
        let using_cores = self.num_using_cores.expect("num_using_cores is None!");
        allocated_cores - using_cores
    }

    pub fn get_minimum_cores(&self) -> i32 {
        self.minimum_cores.expect("minimum_cores is None!")
    }

    pub fn free_allocated_cores(&mut self) {
        match &mut self.num_allocated_cores {
            Some(cores) => *cores = 0,
            None => panic!("num_allocated_cores is None!"),
        }
    }

    pub fn set_minimum_cores(&mut self, minimum_cores: i32) {
        self.minimum_cores = Some(minimum_cores);
    }

    pub fn get_execution_order_head(&self) -> Option<&NodeIndex> {
        if let Some(execution_order) = &self.execution_order {
            execution_order.front()
        } else {
            panic!("execution_order is None!");
        }
    }

    pub fn set_execution_order(&mut self, initial_execution_order: Option<VecDeque<NodeIndex>>) {
        self.initial_execution_order = initial_execution_order.clone();
        self.execution_order = initial_execution_order;
    }

    pub fn allocate_head(&mut self) -> NodeIndex {
        if let Some(execution_order) = &mut self.execution_order {
            if let Some(cores) = self.num_using_cores.as_mut() {
                *cores += 1;
            } else {
                panic!("num_using_cores is not set!");
            }
            execution_order.pop_front().unwrap()
        } else {
            panic!("execution_order is None!");
        }
    }
}

pub fn get_total_allocated_cores(dag_state_managers: &[DAGStateManager]) -> i32 {
    let mut total_allocated_cores = 0;
    for manager in dag_state_managers {
        if let Some(cores) = manager.num_allocated_cores {
            total_allocated_cores += cores;
        }
    }
    total_allocated_cores
}

pub trait DAGSetSchedulerBase<T: ProcessorBase + Clone> {
    fn new(dag_set: &[Graph<NodeData, i32>], processor: &T) -> Self;
    fn initialize(&mut self);
    fn release_dag(&mut self, current_time: i32, log: &mut DAGSetSchedulerLog);
    fn start_dag(&mut self, current_time: i32, log: &mut DAGSetSchedulerLog);
    fn schedule(&mut self) -> i32 {
        self.initialize();
        todo!("Implement this method in the child class");
    }
    fn get_log(&self) -> DAGSetSchedulerLog;
    fn set_log(&mut self, log: DAGSetSchedulerLog);
    fn dump_log(&self, dir_path: &str, alg_name: &str) -> String {
        let file_path = create_scheduler_log_yaml(dir_path, alg_name);
        self.get_log().dump_log_to_yaml(&file_path);

        file_path
    }
}
