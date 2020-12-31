#![feature(trait_alias)]

use bevy::{
    ecs::{Schedule, Stage, ShouldRun},
    prelude::{
        *,
        stage::UPDATE,
    },
};
use std::sync::Mutex;
use std::ops::DerefMut;
use std::collections::hash_map::*;

pub mod stage{
    pub const ROLLBACK_UPDATE: &str = "rollback_update";
}

pub mod logic_stage{
    pub const SAVE_STATE: &str = "save_state";
}

struct RollbackPlugin{
    schedule: Mutex<Option<Schedule>>,
}

impl Plugin for RollbackPlugin{
    fn build(&self, app: &mut AppBuilder){
        app
            .add_stage_before(
                UPDATE,
                stage::ROLLBACK_UPDATE,
                RollbackStage::with_schedule(self.schedule.lock().unwrap().take().unwrap())
            );
    }
}

struct RollbackStage{
    schedule: Schedule,
    run_criteria: Option<Box<dyn System<In = (), Out = ShouldRun>>>,
    run_criteria_initialized: bool,
    
}

impl RollbackStage{
    fn with_schedule(schedule: Schedule) -> Self{
        Self{
            schedule,
            run_criteria: None,
            run_criteria_initialized: false,
        }
    }

    fn new() -> Self{
        Self{
            schedule: Schedule::default(),
            run_criteria: None,
            run_criteria_initialized: false,
        }
    }

    fn run_once(&mut self, world: &mut World, resources: &mut Resources){
        self.schedule.run_once(world, resources);
    }

    fn run_rollback(&mut self, world: &mut World, resources: &mut Resources){
        loop{
            
            let current_state = resources
                .get::<RollbackBuffer>()
                .expect("Couldn't find RollbackBuffer!")
                .rollback_state;

            match current_state{
                RollbackState::Rollback(state) => {
                    // Perform initial rollback
                    // Despawn current rollback scene
                    // Spawn new rollback scene
                    
                    // Setup for catchup
                    resources
                        .get_mut::<RollbackBuffer>()
                        .expect("Couldn't find RollbackBuffer!")
                        .rollback_state = RollbackState::Rolledback(state);
                },
                RollbackState::Rolledback(state) => {
                    // Apply buffered changes for state
                    let changes = resources
                        .get_mut::<RollbackBuffer>()
                        .expect("Couldn't find RollbackBuffer!")
                        .buffered_changes
                        .remove(&state);

                    changes.map(|mut op_vec|
                        for op in op_vec.drain(..){
                            (op)(world, resources);
                        }
                    );

                    // Run schedule for state_n
                    self.run_once(world, resources);
            
                    // Store state_n+1
            
                    // Increment counters
                    let mut rollback_buffer = resources
                        .get_mut::<RollbackBuffer>()
                        .expect("Couldn't find RollbackBuffer!");
                    let new_state = state + 1;
                    rollback_buffer.rollback_state = RollbackState::Rolledback(new_state);
                
                    if new_state == rollback_buffer.newest_frame{
                        // We're all caugt up!
                        break;
                    }
                }
            };
        };
    }
}

impl Stage for RollbackStage{
    fn initialize(&mut self, world: &mut World, resources: &mut Resources){
        if let Some(ref mut run_criteria) = self.run_criteria{
            if !self.run_criteria_initialized{
                run_criteria.initialize(world, resources);
                self.run_criteria_initialized = true;
            }
        }
        self.schedule.initialize(world, resources);
    }

    fn run(&mut self, world: &mut World, resources: &mut Resources){
        loop{
            // Check timestep condition
            let should_run = if let Some(ref mut run_criteria) = self.run_criteria{
                let should_run = run_criteria.run((), world, resources);
                run_criteria.run_thread_local(world, resources);
                should_run.unwrap_or(ShouldRun::No)
            } else {
                ShouldRun::No
            };
            // Check rollback condition during the fixed time step
            match should_run{
                ShouldRun::No => return,
                ShouldRun::Yes=>{
                    resources.get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer!").newest_frame += 1;
                    self.run_rollback(world, resources);
                    return;
                }
                ShouldRun::YesAndLoop => {
                    resources.get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer!").newest_frame += 1;
                    self.run_rollback(world, resources);
                    continue;
                },
            }   
        }
    }
}

#[derive(Copy, Clone)]
enum RollbackState{
    // Rollback to this state
    Rollback(usize),
    // Currently rolledback to this state, this could be the newest frame which is okay
    Rolledback(usize),
}

#[derive(Debug)]
enum RollbackError{
    FrameTimeout,
    ResourceNotFound,
}

trait BufferedChange = FnOnce(&mut World, &mut Resources) -> () + Sync + Send + 'static;

struct RollbackBuffer{
    newest_frame: usize,
    rollback_state: RollbackState,

    buffered_changes: HashMap<usize, Vec<Box<dyn BufferedChange>>>,
    scenes: Vec<Scene>,
    resources: Vec<Resources>,   
}

impl RollbackBuffer{
    pub fn new(buffer_size: usize) -> Self{
        RollbackBuffer{
            newest_frame: 0,
            rollback_state: RollbackState::Rolledback(0),

            buffered_changes: HashMap::new(),
            scenes: Vec::with_capacity(buffer_size),
            resources: Vec::with_capacity(buffer_size),
        }
    }

    pub fn past_frame_change<O: BufferedChange>(&mut self, op: O, frame: usize) -> Result<(), RollbackError>{
        if self.newest_frame - frame >= self.scenes.len(){
            return Err(RollbackError::FrameTimeout);
        }
        match self.buffered_changes.entry(frame){
            Entry::Occupied(mut o) => o.get_mut().push(Box::new(op)),
            Entry::Vacant(v) => {v.insert(vec![Box::new(op)]);},
        };
        Ok(())
    }
}