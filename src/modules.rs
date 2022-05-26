use crossbeam::queue::SegQueue;
use lazy_static::*;
use std::collections::{HashMap, VecDeque};
/// TODO:
///
/// 1. removing CQ?(comparison)
/// that is, the callee module calls a callback function provided by
/// the caller module on completion
///
/// 2. removing module_id, using ref to AsyncModule instead
///
/// 3. proc-macro:
/// #[async_call(async_module1)]
/// fn test(...) {}
/// This mean that we do not need to manually construct the
/// AsyncCallArguments.
///
/// 4. dynamic binding of handler per module
///
/// 5. compatibility with Rust async ecosystem
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

#[derive(Copy, Clone, Debug)]
pub struct AsyncCallArguments {
    pub caller_module_id: usize,
    pub caller_task_id: usize,
    pub callee_module_id: usize,
    pub data: [usize; 5],
}

#[derive(Copy, Clone, Debug)]
pub struct AsyncCallReturnValue {
    pub caller_task_id: usize,
    pub status: usize,
}

pub trait AsyncModule: Send + Sync {
    fn module_id(&self) -> usize;
    fn task_async_call_ret(&self, task_id: usize) -> Option<AsyncCallReturnValue>;
    fn push_sq(&self, args: AsyncCallArguments);
    fn pop_sq(&self) -> Option<AsyncCallArguments>;
    fn push_cq(&self, ret: AsyncCallReturnValue);
    fn pop_cq(&self) -> Option<AsyncCallReturnValue>;
    fn event_loop(&self);
}

pub fn async_call(args: AsyncCallArguments) -> impl Future<Output = AsyncCallReturnValue> {
    struct AsyncCallFuture {
        args: AsyncCallArguments,
    }
    impl AsyncCallFuture {
        fn new(args: AsyncCallArguments) -> Self {
            Self { args }
        }
    }
    impl Future for AsyncCallFuture {
        type Output = AsyncCallReturnValue;
        fn poll(self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Self::Output> {
            println!("into AsyncCallFuture::poll, args = {:?}", self.args);
            let mod_manager = MOD_MANAGER.read().unwrap();
            let caller_mod = mod_manager.get_module(self.args.caller_module_id);
            if let Some(ret) = Arc::clone(&caller_mod).task_async_call_ret(self.args.caller_task_id)
            {
                println!("AsyncCallFuture: Poll::Ready({:?})", ret);
                Poll::Ready(ret)
            } else {
                let callee_mod = MOD_MANAGER
                    .read()
                    .unwrap()
                    .get_module(self.args.callee_module_id);
                println!(
                    "push SQE {:?} to module {}",
                    self.args, self.args.callee_module_id
                );
                callee_mod.push_sq(self.args);
                Poll::Pending
            }
        }
    }
    AsyncCallFuture::new(args)
}

pub struct TaskControlBlock {
    /// TODO: remove Mutex
    pub task: Mutex<Pin<Box<dyn Future<Output = AsyncCallReturnValue> + Sync + Send>>>,
    pub task_id: usize,
    pub args: AsyncCallArguments,
    pub ret: Mutex<Option<AsyncCallReturnValue>>,
}

pub struct Executor {
    pub current_task_id: usize,
    pub ready_queue: VecDeque<Arc<TaskControlBlock>>,
    pub task_table: HashMap<usize, Arc<TaskControlBlock>>,
}

type HandlerBox = Box<dyn Fn((AsyncCallArguments, usize)) -> Pin<Box<dyn Future<Output = AsyncCallReturnValue> + Sync + Send>> + Sync + Send>;

pub struct AsyncModuleImpl {
    // unchanged
    pub module_id: usize,
    pub handler: HandlerBox,
    // Executor: Lock
    pub executor: Mutex<Executor>,
    // SQ & CQ: Lock-free
    pub sq: SegQueue<AsyncCallArguments>,
    // Removing CQ?
    pub cq: SegQueue<AsyncCallReturnValue>,
}

impl AsyncModule for AsyncModuleImpl {
    fn module_id(&self) -> usize {
        self.module_id
    }

    fn push_sq(&self, args: AsyncCallArguments) {
        self.sq.push(args);
    }

    fn pop_sq(&self) -> Option<AsyncCallArguments> {
        self.sq.pop()
    }

    fn push_cq(&self, ret: AsyncCallReturnValue) {
        self.cq.push(ret);
    }

    fn pop_cq(&self) -> Option<AsyncCallReturnValue> {
        self.cq.pop()
    }

    fn task_async_call_ret(&self, task_id: usize) -> Option<AsyncCallReturnValue> {
        self.executor
            .lock()
            .unwrap()
            .task_table
            .get(&task_id)
            .unwrap()
            .ret
            .lock()
            .unwrap()
            .take()
    }

    fn event_loop(&self) {
        loop {
            // scanning SQ -> try to spawn new tasks
            while let Some(args) = self.pop_sq() {
                //println!("event_loop1 from SQ: args = {:?}", args);
                self.spawn_task(args);
            }
            // scanning CQ -> try tp wake up blocked tasks
            while let Some(ret) = self.pop_cq() {
                //println!("event_loop1 from CQ: ret = {:?}", ret);
                let mut executor = self.executor.lock().unwrap();
                // update ret in the TCB so that AsyncCallFuture::poll
                // will return Poll::Ready
                let task = executor
                    .task_table
                    .get(&ret.caller_task_id)
                    .unwrap()
                    .clone();
                *task.ret.lock().unwrap() = Some(ret);
                executor.ready_queue.push_back(Arc::clone(&task));
            }
            let task = self.executor.lock().unwrap().ready_queue.pop_front();
            if let Some(task) = task {
                let raw_waker = RawWaker::new(core::ptr::null(), &EMPTY_WAKER_TABLE);
                let waker = unsafe { Waker::from_raw(raw_waker) };
                let mut cx = Context::from_waker(&waker);
                //let task = task.lock().unwrap().task.clone();
                let caller_module_id = task.args.caller_module_id;
                if let Poll::Ready(ret) = task.task.lock().unwrap().as_mut().poll(&mut cx) {
                    // as an output module, here we do not need to write to CQ
                    // we can just print some info
                    println!("event_loop: ret = {:?}", ret);
                    MOD_MANAGER
                        .write()
                        .unwrap()
                        .get_module(caller_module_id)
                        .push_cq(ret);
                    // TODO: remove this task from the task table
                } // else: do nothing, do not put the task back to the ready_queue
            }
        }
    }
}

impl AsyncModuleImpl {
    pub fn new(module_id: usize, handler: HandlerBox) -> Self {
        Self {
            module_id,
            handler,
            executor: Mutex::new(Executor {
                current_task_id: 0,
                ready_queue: VecDeque::new(),
                task_table: HashMap::new(),
            }),
            sq: SegQueue::new(),
            cq: SegQueue::new(),
        }
    }

    pub fn spawn_task(&self, args: AsyncCallArguments) {
        let mut executor = self.executor.lock().unwrap();
        executor.current_task_id += 1;
        let current_task_id = executor.current_task_id;
        let tcb = Arc::new(TaskControlBlock {
            task: Mutex::new((self.handler)((args, current_task_id))),
            task_id: current_task_id,
            args,
            ret: Mutex::new(None),
        });
        executor.ready_queue.push_back(Arc::clone(&tcb));
        executor.task_table.insert(current_task_id, tcb);
    }
}

/*
impl AsyncModule2 {
    pub fn event_loop(&self) {
        loop {
            // scanning SQ -> try to spawn new tasks
            while let Some(args) = self.pop_sq() {
                println!("event_loop2 from SQ: args = {:?}", args);
                self.spawn_task(args);
            }
            // scanning CQ -> try tp wakeup blocked tasks
            while let Some(ret) = self.pop_cq() {
                println!("event_loop2 from CQ: ret = {:?}", ret);
                let mut executor = self.executor.lock().unwrap();
                // update ret in the TCB so that AsyncCallFuture::poll
                // will return Poll::Ready
                let task = executor
                    .task_table
                    .get(&ret.caller_task_id)
                    .unwrap()
                    .clone();
                *task.ret.lock().unwrap() = Some(ret);
                executor.ready_queue.push_back(Arc::clone(&task));
            }
            if let Some(task) = self.executor.lock().unwrap().ready_queue.pop_front() {
                let raw_waker = RawWaker::new(core::ptr::null(), &EMPTY_WAKER_TABLE);
                let waker = unsafe { Waker::from_raw(raw_waker) };
                let mut cx = Context::from_waker(&waker);
                let caller_module_id = task.args.caller_module_id;
                if let Poll::Ready(ret) = task.task.lock().unwrap().as_mut().poll(&mut cx) {
                    // here we need to write to CQ
                    MOD_MANAGER
                        .write()
                        .unwrap()
                        .get_module(caller_module_id)
                        .push_cq(ret);
                    // TODO: remove this task from the task table
                } // else: do nothing, do not put the task back to the ready_queue
            }
        }
    }
}
*/
pub struct AsyncModuleManager {
    //current_module_id: usize,
    module_table: HashMap<usize, Arc<dyn AsyncModule>>,
}

impl AsyncModuleManager {
    pub fn new() -> Self {
        Self {
            /*current_module_id: 0, */ module_table: HashMap::new(),
        }
    }

    pub fn get_module(&self, module_id: usize) -> Arc<dyn AsyncModule> {
        self.module_table.get(&module_id).unwrap().clone()
    }

    /*
    pub fn alloc_module_id(&mut self) -> usize {
        self.current_module_id += 1;
        self.current_module_id
    }
    */

    pub fn load_module(&mut self, module_id: usize, module: Arc<dyn AsyncModule>) {
        self.module_table.insert(module_id, module);
    }
}

lazy_static! {
    pub static ref MOD_MANAGER: RwLock<AsyncModuleManager> = RwLock::new(AsyncModuleManager::new());
}

/// TODO: It does not make sense now!
const EMPTY_WAKER_TABLE: RawWakerVTable = RawWakerVTable::new(
    |data| RawWaker::new(data, &EMPTY_WAKER_TABLE),
    |_data| {},
    |_data| {},
    |_data| {},
);
