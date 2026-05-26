#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskGroupState {
    Running,
    Cancelling,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskExit {
    Clean,
    Failed(crate::model::ErrorCode),
}

#[derive(Debug, Clone)]
pub struct TaskGroup {
    state: TaskGroupState,
    exit: Option<TaskExit>,
}

impl TaskGroup {
    pub fn new_for_tests() -> Self {
        Self {
            state: TaskGroupState::Running,
            exit: None,
        }
    }

    pub fn state(&self) -> TaskGroupState {
        self.state
    }

    pub fn cancel(&mut self) {
        if self.state == TaskGroupState::Running {
            self.state = TaskGroupState::Cancelling;
        }
    }

    pub fn finish(&mut self, exit: TaskExit) {
        self.exit = Some(exit);
        self.state = TaskGroupState::Finished;
    }

    pub fn exit(&self) -> Option<TaskExit> {
        self.exit
    }
}
