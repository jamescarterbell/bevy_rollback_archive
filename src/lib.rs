use bevy::{
    ecs::{Schedule, Stage, ShouldRun},
    prelude::{
        *,
        stage::UPDATE,
    },
};
use std::sync::Mutex;

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
    rollback_run_criteria: Option<Box<dyn System<In = (), Out = ShouldRun>>>,
    rollback_run_criteria_initialized: bool,
    run_criteria: Option<Box<dyn System<In = (), Out = ShouldRun>>>,
    run_criteria_initialized: bool,
    
}

impl RollbackStage{
    fn with_schedule(schedule: Schedule) -> Self{
        Self{
            schedule,
            rollback_run_criteria: None,
            rollback_run_criteria_initialized: false,
            run_criteria: None,
            run_criteria_initialized: false,
        }
    }

    fn new() -> Self{
        Self{
            schedule: Schedule::default(),
            rollback_run_criteria: None,
            rollback_run_criteria_initialized: false,
            run_criteria: None,
            run_criteria_initialized: false,
        }
    }

    fn run_once(&mut self, world: &mut World, resource: &mut Resources){

    }
}

impl Stage for RollbackStage{
    fn initialize(&mut self, world: &mut World, resources: &mut Resources){
        if let Some(ref mut rollback_run_criteria) = self.rollback_run_criteria{
            if !self.rollback_run_criteria_initialized{
                rollback_run_criteria.initialize(world, resources);
                self.rollback_run_criteria_initialized = true;
            }
        }
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
            // Check rollback condition
            let should_run = if let Some(ref mut rollback_run_criteria) = self.rollback_run_criteria{
                let should_run = rollback_run_criteria.run((), world, resources);
                rollback_run_criteria.run_thread_local(world, resources);
                should_run.unwrap_or(ShouldRun::No)
            } else {
                ShouldRun::No
            };
            // Check timestep condition after we're done rolling back
            match should_run{
                ShouldRun::No=>{
                    let should_run = if let Some(ref mut run_criteria) = self.run_criteria{
                        let should_run = run_criteria.run((), world, resources);
                        run_criteria.run_thread_local(world, resources);
                        should_run.unwrap_or(ShouldRun::No)
                    } else {
                        ShouldRun::Yes
                    };

                    match should_run{
                        ShouldRun::No => return,
                        ShouldRun::Yes => {
                            self.run_once(world, resources);
                            return;
                        }
                        ShouldRun::YesAndLoop => {
                            self.run_once(world, resources);
                        }
                    }
                }
                ShouldRun::Yes=>{
                    self.run_once(world, resources);
                    return;
                }
                ShouldRun::YesAndLoop => self.run_once(world, resources),
            }   
        }
    }
}