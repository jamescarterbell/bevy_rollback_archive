use bevy::prelude::*;
use bevy::ecs::Command;
use crate::RollbackBuffer;
use std::ops::{Deref, DerefMut};
use std::cell::RefCell;

pub struct LogicCommandsBuilder{
    pub commands: Commands
}

impl LogicCommandsBuilder{
    pub fn new(rollback_buffer: &RollbackBuffer) -> Self{
        let mut logic_commands = LogicCommandsBuilder{
            commands: Commands::default()
        };
        logic_commands.commands.set_entity_reserver(rollback_buffer.current_world.get_entity_reserver());
        logic_commands
    }

    pub fn build(self) -> LogicCommands{
        LogicCommands{
            commands: RefCell::new(self.commands)
        }
    }
}

pub struct LogicCommands{
    commands: RefCell<Commands>
}

unsafe impl Send for LogicCommands{}
unsafe impl Sync for LogicCommands{}

impl Command for LogicCommands{
    fn write(self: Box<Self>, _world: &mut World, resources: &mut Resources){
        let mut rollback_buffer_r = resources.get_mut::<RollbackBuffer>().expect("Couldn't find RollbackBuffer!");
        let mut rollback_buffer = rollback_buffer_r.deref_mut();
        self.commands.borrow_mut().apply(&mut rollback_buffer.current_world, &mut rollback_buffer.current_resources);
    }
}
