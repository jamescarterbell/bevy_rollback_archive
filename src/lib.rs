#![feature(trait_alias)]

mod res;
mod query;
mod commands;

use bevy::{
    ecs::{Schedule, Stage, ShouldRun, Archetype},
    prelude::{
        *,
        stage::UPDATE,
    },
    reflect::TypeRegistryArc,
    scene::serde::SceneSerializer,
};
use std::ops::DerefMut;
use std::collections::hash_map::*;
use std::any::TypeId;
use std::sync::{Arc, Mutex};

pub use res::{LRes, LResMut};
pub use query::{LQuery};
pub use commands::{LogicCommands, LogicCommandsBuilder};

pub mod stage{
    pub const ROLLBACK_UPDATE: &str = "rollback_update";
}

pub mod logic_stages{
    pub const SAVE_STATE: &str = "save_state";
    pub const LOGIC_UPDATE: &str = "logic_update";
    pub const LOGIC_PREUPDATE: &str = "logic_preupdate";
    pub const LOGIC_POSTUPDATE: &str = "logic_postupdate";
}

pub struct RollbackPlugin{
    schedule: Mutex<Option<Schedule>>,
    buffer_size: usize,
    run_criteria: Mutex<Option<Box<dyn System<In = (), Out = ShouldRun>>>>,
}

impl RollbackPlugin{
    pub fn with_schedule(buffer_size: usize, schedule: Schedule) -> Self{
        Self{
            schedule: Mutex::new(Some(schedule)),
            buffer_size,
            run_criteria: Mutex::new(None),
        }
    }

    pub fn with_buffer_size(buffer_size: usize) -> Self{
        Self{
            schedule: Mutex::new(None),
            buffer_size,
            run_criteria: Mutex::new(None),
        }
    }

    pub fn with_run_criteria<S: System<In = (), Out = ShouldRun>>(mut self, system: S) -> Self {
        self.run_criteria = Mutex::new(Some(Box::new(system)));
        self
    }
}

impl Plugin for RollbackPlugin{
    fn build(&self, app: &mut AppBuilder){
        let rollback_buffer = RollbackBuffer::new(
            self.buffer_size
        );

        {
            let mut registry = rollback_buffer.logic_registry.write();
            registry.register::<bool>();
            registry.register::<u8>();
            registry.register::<u16>();
            registry.register::<u32>();
            registry.register::<u64>();
            registry.register::<u128>();
            registry.register::<usize>();
            registry.register::<i8>();
            registry.register::<i16>();
            registry.register::<i32>();
            registry.register::<i64>();
            registry.register::<i128>();
            registry.register::<isize>();
            registry.register::<f32>();
            registry.register::<f64>();
            registry.register::<String>();
            #[cfg(feature = "glam")]
            {
                registry.register::<glam::Vec2>();
                registry.register::<glam::Vec3>();
                registry.register::<glam::Vec4>();
                registry.register::<glam::Mat3>();
                registry.register::<glam::Mat4>();
                registry.register::<glam::Quat>();
            }
            #[cfg(feature = "bevy_ecs")]
            {
                registry.register::<bevy_ecs::Entity>();
            }
        }

        let run_criteria = self.run_criteria.lock().unwrap().take();

        
        let mut stage = {
            if let Some(schedule) = self.schedule.lock().unwrap().take(){
                RollbackStage::with_schedule(schedule)
            }
            else{
                RollbackStage::new()
            }
        };
        
        stage.run_criteria = run_criteria;
        stage.run_criteria_initialized = false;


        app
            .add_resource(
                rollback_buffer
            )
            .add_stage_before(
                UPDATE,
                stage::ROLLBACK_UPDATE,
                stage
            );
    }
}

pub struct RollbackStage{
    schedule: Schedule,
    initialized: bool,
    run_criteria: Option<Box<dyn System<In = (), Out = ShouldRun>>>,
    run_criteria_initialized: bool,
}

impl RollbackStage{
    pub fn with_schedule(schedule: Schedule) -> Self{
        Self{
            schedule,
            initialized: false,
            run_criteria: None,
            run_criteria_initialized: false,
        }
    }

    pub fn new() -> Self{
        Self{
            schedule: Schedule::default()
                .with_stage(logic_stages::LOGIC_UPDATE, SystemStage::parallel())
                .with_stage_before(logic_stages::LOGIC_UPDATE, logic_stages::LOGIC_PREUPDATE, SystemStage::parallel())
                .with_stage_after(logic_stages::LOGIC_UPDATE, logic_stages::LOGIC_POSTUPDATE, SystemStage::parallel()),
            initialized: false,
            run_criteria: None,
            run_criteria_initialized: false,
        }
    }

    pub fn run_once(&mut self, world: &mut World, resources: &mut Resources, state: usize){
        let mut rollback_buffer_r = resources
                .get_mut::<RollbackBuffer>()
                .expect("Couldn't find RollbackBuffer!");
        let mut rollback_buffer = rollback_buffer_r   
                .deref_mut();

        let target = state % rollback_buffer.past_resources.len();

        let changes = rollback_buffer.buffered_changes.lock().unwrap().remove(&state);

        let current_world = &mut rollback_buffer.current_world;
        let current_resources = &mut rollback_buffer.current_resources;

        // Apply changes
        if let Some(mut changes) = changes{
            changes.run_once(
                current_world,
                current_resources,
            );
        }

        // Apply overrides
        for override_fn in rollback_buffer.resource_override.iter(){
            (override_fn)(&mut rollback_buffer.current_resources, rollback_buffer.past_resources.get(target).unwrap().as_ref().unwrap());
        }
        
        drop(rollback_buffer);
        drop(rollback_buffer_r);

        // Store everything
        store_new_resources(resources, state + 1);
        store_new_world(resources, state + 1);

        let mut rollback_buffer_r = resources
                .get_mut::<RollbackBuffer>()
                .expect("Couldn't find RollbackBuffer!");
                
        let mut rollback_buffer = rollback_buffer_r   
            .deref_mut();

        // Run the schedule
        self.schedule.run_once(&mut rollback_buffer.current_world, &mut rollback_buffer.current_resources);
        
    }

    pub fn run_rollback(&mut self, world: &mut World, resources: &mut Resources){
        loop{
            
            let current_state = resources
                .get::<RollbackBuffer>()
                .expect("Couldn't find RollbackBuffer!")
                .rollback_state
                .lock()
                .unwrap()
                .clone();

            match current_state{
                RollbackState::Rollback(state) => {
                    // Literally just swap the worlds
                    let mut rollback_buffer = resources
                        .get_mut::<RollbackBuffer>()
                        .expect("Couldn't find RollbackBuffer!");

                    let target = rollback_buffer.newest_frame % 
                        rollback_buffer
                            .past_worlds
                            .len();

                    rollback_buffer
                        .current_world = rollback_buffer
                            .past_worlds
                            .get_mut(target)
                            .unwrap()
                            .take()
                            .expect("Frame doesn't exist!");

                    rollback_buffer
                        .current_resources = rollback_buffer
                            .past_resources
                            .get_mut(target)
                            .unwrap()
                            .take()
                            .expect("Frame doesn't exist!");

                    // Setup for catchup
                    *rollback_buffer
                        .rollback_state
                        .lock()
                        .unwrap() = RollbackState::Rolledback(state);
                },
                RollbackState::Rolledback(state) => {
                    // Run schedule for state_n
                    self.run_once(world, resources, state);
                    
                    let mut rollback_buffer = resources
                        .get_mut::<RollbackBuffer>()
                        .expect("Couldn't find RollbackBuffer!");
            
                    // Increment counters
                    match state{
                        state if state >= rollback_buffer.newest_frame =>{
                            *rollback_buffer.rollback_state.lock().unwrap() = RollbackState::Rolledback(state + 1);
                            // We're all caugt up!
                            break;
                        }
                        _ => *rollback_buffer.rollback_state.lock().unwrap() = RollbackState::Rolledback(state + 1),
                    }
                }
            };
        };
    }

    pub fn with_run_criteria<S: System<In = (), Out = ShouldRun>>(mut self, system: S) -> Self {
        self.set_run_criteria(system);
        self
    }

    pub fn set_run_criteria<S: System<In = (), Out = ShouldRun>>(
        &mut self,
        system: S,
    ) -> &mut Self {
        self.run_criteria = Some(Box::new(system.system()));
        self.run_criteria_initialized = false;
        self
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
        

        if !self.initialized{
            self.initialized = true;
            
            store_new_resources(resources, 0);
            store_new_world(resources, 0);
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
                    resources.get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer").newest_frame += 1;
                    self.run_rollback(world, resources);
                    return;
                }
                ShouldRun::YesAndLoop => {
                    resources.get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer").newest_frame += 1;
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
pub enum RollbackError{
    FrameTimeout,
    ResourceNotFound,
}

pub trait ResourceRollbackFn = Fn(&mut Resources, &Resources) -> () + Sync + Send;

pub struct RollbackBuffer{
    pub newest_frame: usize,
    rollback_state: Arc<Mutex<RollbackState>>,

    pub(crate) current_world: World,
    pub(crate) current_resources: Resources,

    buffered_changes: Arc<Mutex<HashMap<usize, SystemStage>>>,

    past_worlds: Vec<Option<World>>,
    past_resources: Vec<Option<Resources>>,   

    resource_rollback: Vec<Box<dyn ResourceRollbackFn>>,
    resource_override: Vec<Box<dyn ResourceRollbackFn>>,

    pub logic_registry: TypeRegistryArc,
}

impl RollbackBuffer{
    pub fn new(buffer_size: usize) -> Self{
        RollbackBuffer{
            newest_frame: 0,
            rollback_state: Arc::new(Mutex::new(RollbackState::Rolledback(0))),
            
            current_world: World::new(),
            current_resources: Resources::default(),

            buffered_changes: Arc::new(Mutex::new(HashMap::new())),

            past_worlds: (0..buffer_size).map(|_| None).collect(),
            past_resources: (0..buffer_size).map(|_| None).collect(),

            resource_rollback: Vec::new(),
            resource_override: Vec::new(),

            logic_registry: TypeRegistryArc::default(),
        }
    }

    pub fn past_frame_change<S: System<In = (), Out = ()>>(&self, frame: usize, op: S) -> Result<(), RollbackError>{
        if self.newest_frame - frame >= self.past_worlds.len(){
            return Err(RollbackError::FrameTimeout);
        }
        match self.buffered_changes.lock().unwrap().entry(frame){
            Entry::Occupied(mut o) => o.get_mut().add_system(op),
            Entry::Vacant(v) => v.insert({
                let mut stage = SystemStage::parallel();
                stage.add_system(op);
                stage
            }),
        };
        let mut rollback_state = self.rollback_state.lock().unwrap();
        *rollback_state = match *rollback_state{
            RollbackState::Rolledback(cur) => RollbackState::Rollback(frame),
            RollbackState::Rollback(cur) if frame < cur => RollbackState::Rollback(frame),
            RollbackState::Rollback(cur) => RollbackState::Rollback(cur),
        };
        Ok(())
    }

    pub fn get_logic_commands_builder(&self) -> LogicCommandsBuilder{
        LogicCommandsBuilder::new(self)
    }
}

pub struct RollbackTracked;

fn store_new_world(resources: &mut Resources, state: usize){
    let mut rollback_buffer_r = resources
                .get_mut::<RollbackBuffer>()
                .expect("Couldn't find RollbackBuffer!");
                
        let mut rollback_buffer = rollback_buffer_r   
            .deref_mut();
        
    let mut world = &mut rollback_buffer
        .current_world;
    
    let resources = &rollback_buffer
        .current_resources;

    let mut new_world = World::new();


    let type_registry = rollback_buffer.logic_registry.read();


    for archetype in world.archetypes(){
        for (index, entity) in archetype.iter_entities().enumerate() {
            // Reserve the new entity in the world then allocate space for it in the Archetype
            let new_entity = new_world.reserve_entity();

            // Copy over component data to the new entity with the power of friendship
            for type_info in archetype.types() {
                if let Some(registration) = type_registry.get(type_info.id()) {
                    if let Some(reflect_component) = registration.data::<ReflectComponent>() {
                        let comp = unsafe{
                            reflect_component
                                .reflect_component(
                                    archetype,
                                    index
                                )
                        };
                        reflect_component
                            .add_component(
                                &mut new_world,
                                resources,
                                *entity,
                                comp
                        );
                    }
                }
            }
        }
    }

    let buffer_pos = state %
        rollback_buffer
            .past_resources
            .len();

    *rollback_buffer
        .past_worlds
        .get_mut(buffer_pos)
        .expect("RollbackBuffer Index is out of bounds!") = Some(new_world);      
}

fn store_new_resources(resources: &mut Resources, state: usize){
    let mut rollback_buffer = resources
        .get_mut::<RollbackBuffer>()
        .expect("Couldn't find RollbackBuffer!");

    let mut new_resources = Resources::default();

    for resource_rollback_fn in rollback_buffer.resource_rollback.iter(){
        (resource_rollback_fn)(&mut new_resources, &rollback_buffer.current_resources);
    }

    // Since new_resources is an exact copy of the current_resources, we can just store new_resources
    // This should also drop the old resources question mark?

    let buffer_pos = state %
        rollback_buffer
            .past_resources
            .len();

    *rollback_buffer
        .past_resources
        .get_mut(buffer_pos)
        .expect("RollbackBuffer Index is out of bounds!") = Some(new_resources);  
}

pub trait ResourceTracker{
    fn track_resource<R: Resource + Clone +>(&mut self, resource: R) -> &mut Self;
    fn override_resource<R: Resource + Clone>(&mut self, resource: R) -> &mut Self;
}

impl ResourceTracker for AppBuilder{
    fn track_resource<R: Resource + Clone>(&mut self, resource: R) -> &mut Self{
        {
            let mut rollback_buffer = self.resources().get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer!");

            rollback_buffer.current_resources.insert(resource);

            rollback_buffer
                .resource_rollback
                .push(
                    Box::new(|dest_res: &mut Resources, res: &Resources|{
                        dest_res.insert(res.get_cloned::<R>().unwrap());
                    })
            );
        }
        self
    }

    fn override_resource<R: Resource + Clone>(&mut self, resource: R) -> &mut Self{
        {
            let mut rollback_buffer = self.resources().get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer!");

            rollback_buffer.current_resources.insert(resource);

            rollback_buffer
                .resource_rollback
                .push(
                Box::new(|dest_res: &mut Resources, res: &Resources|{
                    dest_res.insert(res.get_cloned::<R>().unwrap());
                })
            );

            rollback_buffer
                .resource_override
                .push(
                Box::new(|dest_res: &mut Resources, res: &Resources|{
                    dest_res.insert(res.get_cloned::<R>().unwrap());
                })
            );
        }
        self
    }
}

pub trait RollbackStageUtil{
    fn add_logic_system<S: System<In = (), Out = ()>>(&mut self, system: S) -> &mut AppBuilder;
    fn add_logic_system_to_stage<S: System<In = (), Out = ()>>(&mut self, stage_name: &'static str, system: S) -> &mut AppBuilder;
    fn add_logic_stage<S: Stage>(&mut self, name: &str, stage: S) -> &mut AppBuilder;
    fn add_logic_stage_after<S: Stage>(&mut self, target: &str, name: &str, stage: S) -> &mut AppBuilder;
    fn add_logic_stage_before<S: Stage>(&mut self, target: &str, name: &str, stage: S) -> &mut AppBuilder;
}

impl RollbackStageUtil for AppBuilder{


    fn add_logic_system<S: System<In = (), Out = ()>>(&mut self, system: S) -> &mut AppBuilder{
        self
            .app
            .schedule
            .get_stage_mut::<RollbackStage>(stage::ROLLBACK_UPDATE)
            .expect("Add RollbackStage to app!")
            .schedule
            .add_system_to_stage(
                logic_stages::LOGIC_UPDATE,
                system
            );
        self
    }

    fn add_logic_system_to_stage<S: System<In = (), Out = ()>>(&mut self, stage_name: &'static str, system: S) -> &mut AppBuilder{
        self
            .app
            .schedule
            .get_stage_mut::<RollbackStage>(stage::ROLLBACK_UPDATE)
            .expect("Add RollbackStage to app!")
            .schedule
            .add_system_to_stage(
                stage_name,
                system
            );
        self
    }

    fn add_logic_stage<S: Stage>(&mut self, name: &str, stage: S) -> &mut AppBuilder{
        self
            .app
            .schedule
            .get_stage_mut::<RollbackStage>(stage::ROLLBACK_UPDATE)
            .expect("Add RollbackStage to app!")
            .schedule
            .add_stage(
                name,
                stage
            );
        self
    }

    fn add_logic_stage_after<S: Stage>(&mut self, target: &str, name: &str, stage: S) -> &mut AppBuilder{
        self
            .app
            .schedule
            .get_stage_mut::<RollbackStage>(stage::ROLLBACK_UPDATE)
            .expect("Add RollbackStage to app!")
            .schedule
            .add_stage_after(
                target,
                name,
                stage
            );
        self
    }

    fn add_logic_stage_before<S: Stage>(&mut self, target: &str, name: &str, stage: S) -> &mut AppBuilder{
        self
            .app
            .schedule
            .get_stage_mut::<RollbackStage>(stage::ROLLBACK_UPDATE)
            .expect("Add RollbackStage to app!")
            .schedule
            .add_stage_before(
                target,
                name,
                stage
            );
        self
    }
}