use super::NMPProcessor;
use crate::{
    simulate::{memory::DataCache, nmpgc::NMPGC},
    trace::trace_object,
    *,
};

#[derive(Debug, Clone)]
/// Each processor generates at most one message per tick
pub(super) struct NMPMessage {
    pub(super) recipient: usize,
    work: NMPMessageWork,
}

#[derive(Debug, Clone)]
pub(super) enum NMPMessageWork {
    Mark(u64),
    Load(*mut u64),
}

#[derive(Debug, Clone)]
pub(super) enum NMPProcessorWork {
    Mark(u64),
    Load(*mut u64),
    Idle,
    ReadInbox,
    SendMessage(NMPMessage),
    ContinueScan,
}

#[repr(u8)]
#[derive(Debug, Clone, PartialEq, Hash, Eq)]
pub(super) enum NMPProcessorWorkType {
    Mark = 0,
    Load = 1,
    Idle = 2,
    ReadInbox = 3,
    SendMessage = 4,
    ContinueScan = 5,
}

impl NMPProcessorWork {
    fn get_type(&self) -> NMPProcessorWorkType {
        match self {
            NMPProcessorWork::Mark(_) => NMPProcessorWorkType::Mark,
            NMPProcessorWork::Load(_) => NMPProcessorWorkType::Load,
            NMPProcessorWork::Idle => NMPProcessorWorkType::Idle,
            NMPProcessorWork::ReadInbox => NMPProcessorWorkType::ReadInbox,
            NMPProcessorWork::SendMessage(_) => NMPProcessorWorkType::SendMessage,
            NMPProcessorWork::ContinueScan => NMPProcessorWorkType::ContinueScan,
        }
    }
}

impl NMPProcessor {
    pub(super) fn tick<O: ObjectModel>(&mut self) -> Option<NMPMessage> {
        self.ticks += 1;

        // This is to deal with the latencies of actions that take more than one tick
        if self.stall_ticks > 0 {
            self.stall_ticks -= 1;
            self.busy_ticks += 1;
            trace!(
                "[P{}] stalled for {:?}, {} ticks left",
                self.id,
                self.stalled_work,
                self.stall_ticks
            );
            return None;
        }

        let work = if let Some(w) = self.stalled_work.take() {
            trace!("[P{}] executing previously stalled work: {:?}", self.id, w);
            // Act on the stalled work
            w
        } else {
            if let Some(w) = self.works.pop_front() {
                if self.get_latency(&w) > 1 {
                    // If the work takes more than one tick, stall it
                    self.stall_ticks = self.get_latency(&w) - 1; // -1 because we are already in this tick
                    self.stalled_work = Some(w);
                    trace!(
                        "[P{}] stalling work: {:?}, {} ticks left",
                        self.id,
                        self.stalled_work,
                        self.stall_ticks
                    );
                    return None;
                } else {
                    w
                }
            } else {
                NMPProcessorWork::Idle
            }
        };

        if !matches!(work, NMPProcessorWork::Idle) {
            self.busy_ticks += 1;
        }

        if !(matches!(work, NMPProcessorWork::Idle) || matches!(work, NMPProcessorWork::ReadInbox))
        {
            // This processor is doing productive work now
            if let Some(start) = self.idle_start.take() {
                self.idle_ranges.push((start, self.ticks - 1));
            }
        }

        let mut ret = None;
        self.work_count
            .entry(work.get_type())
            .and_modify(|e| *e += 1)
            .or_insert(1);
        match work {
            NMPProcessorWork::Mark(o) => {
                trace!("[P{}] marking object {}", self.id, o);
                if unsafe { trace_object(o, 1) } {
                    self.cache.write(self.mem_config, o);
                    self.marked_objects += 1;
                    O::scan_object(o, |edge, repeat| {
                        // To avoid edges getting dereferenced when there's no edge
                        if repeat > 0 {
                            self.edge_chunks.push((edge as u64, repeat));
                        }
                    });
                    self.edge_chunk_cursor = (0, 0);
                    if !self.edge_chunks.is_empty() {
                        // To make sure we finish scanning the current object first
                        // Otherwise, we might end up doing other work, such as loading edges and marking objects
                        // and disrupts the current scanning process
                        self.works.push_front(NMPProcessorWork::ContinueScan);
                    }
                }
            }
            NMPProcessorWork::Load(e) => {
                let child = unsafe { *e };
                self.cache.read(self.mem_config, e as u64);
                if child != 0 {
                    let owner = self.mem_config.get_owner_processor(child);
                    if owner as usize == self.id {
                        self.works.push_back(NMPProcessorWork::Mark(child));
                    } else {
                        let msg = NMPMessage {
                            recipient: owner as usize,
                            work: NMPMessageWork::Mark(child),
                        };
                        self.works.push_back(NMPProcessorWork::SendMessage(msg));
                    }
                }
            }
            NMPProcessorWork::Idle => {
                if !self.inbox.is_empty() {
                    self.idle_readinbox_ticks += 1;
                    self.works.push_back(NMPProcessorWork::ReadInbox);
                } else {
                    // This process is truly idle
                    if self.idle_start.is_none() {
                        self.idle_start = Some(self.ticks);
                    }
                }
            }
            NMPProcessorWork::SendMessage(msg) => {
                trace!(
                    "[P{}] sending message to P{}: {:?}",
                    self.id,
                    msg.recipient,
                    msg.work
                );
                ret = Some(msg);
            }
            NMPProcessorWork::ReadInbox => {
                if let Some(msg) = self.inbox.pop() {
                    trace!("[P{}] reading inbox message: {:?}", self.id, msg);
                    match msg.work {
                        NMPMessageWork::Load(e) => {
                            self.works.push_back(NMPProcessorWork::Load(e));
                        }
                        NMPMessageWork::Mark(o) => {
                            self.works.push_back(NMPProcessorWork::Mark(o));
                        }
                    }
                }
            }
            NMPProcessorWork::ContinueScan => {
                let (chunk_idx, edge_idx) = self.edge_chunk_cursor;
                let (first_edge_in_chunk, edges_in_chunk) =
                    *self.edge_chunks.get(chunk_idx).unwrap();
                let e = (first_edge_in_chunk as *mut u64).wrapping_add(edge_idx as usize);
                let owner = self.mem_config.get_owner_processor(e as u64);
                if owner as usize == self.id {
                    self.works.push_back(NMPProcessorWork::Load(e));
                } else {
                    // Eagerly publish work so others have work to do
                    self.works
                        .push_front(NMPProcessorWork::SendMessage(NMPMessage {
                            recipient: owner as usize,
                            work: NMPMessageWork::Load(e),
                        }));
                }
                if edge_idx + 1 < edges_in_chunk {
                    // Move to the next edge in the current chunk
                    self.edge_chunk_cursor = (chunk_idx, edge_idx + 1);
                    self.works.push_front(NMPProcessorWork::ContinueScan);
                } else if chunk_idx + 1 < self.edge_chunks.len() {
                    // Move to the next chunk
                    self.edge_chunk_cursor = (chunk_idx + 1, 0);
                    self.works.push_front(NMPProcessorWork::ContinueScan);
                } else {
                    // No more edges to process
                    self.edge_chunks.clear();
                    self.edge_chunk_cursor = (0, 0);
                }
            }
        }
        trace!(
            "[P{}] work count: {:?}, inbox count: {:?}, stalled_work: {:?}, marked_objects: {:?}",
            self.id,
            self.works.len(),
            self.inbox.len(),
            self.stalled_work,
            self.marked_objects
        );
        ret
    }
}
