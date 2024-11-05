use crate::*;
use anyhow::Result;

mod heap_dump;
mod linked_list;
mod physical;

trait MemdumpWorkload {
    unsafe fn gen_memdump<O: ObjectModel, M: Memdump>(
        &self,
        object_model: O,
        args: Args,
        md: &mut M,
    );
}

pub(crate) trait Memdump {
    type MM: MemdumpMapping;
    type MI: MemoryInterface;
    fn new_mapping(&mut self, host_addr: *mut u8, size: usize) -> Self::MM;
    fn gen_memif(&self) -> Self::MI;
    unsafe fn dump_to_file(&self, output: &str);
}

pub(crate) trait MemdumpMapping {
    fn to_arena(&self, align: usize) -> BumpAllocationArena;
}

// Such as PATH=$HOME/protoc/bin:$PATH cargo run --release -- ../heapdumps/sampled/luindex/* -o OpenJDK memdump --workload LinkedList --output ./1.bin --mem-start 0xc0000000
pub fn dump_mem<O: ObjectModel>(object_model: O, args: Args) -> Result<()> {
    let memdump_args = if let Some(Commands::Memdump(ref a)) = args.command {
        a.clone()
    } else {
        panic!("Incorrect dispatch");
    };
    unsafe {
        let mut memdump = physical::MemdumpPhysical::new(
            1024usize * 1024 * 1024 * 4,
            memdump_args.mem_base as *mut u8,
        );
        match memdump_args.workload {
            cli::MemdumpWorkload::LinkedList => {
                linked_list::LinkedList::new(1024).gen_memdump(object_model, args, &mut memdump)
            }
            cli::MemdumpWorkload::HeapDump => {
                heap_dump::HeapDumpWorkload::new().gen_memdump(object_model, args, &mut memdump)
            }
            cli::MemdumpWorkload::HeapLinkedList => linked_list::HeapLinkedList::new(
                1024, 1, 6, None,
            )
            .gen_memdump(object_model, args, &mut memdump),
        }
        memdump.dump_to_file(&memdump_args.output);
    }
    Ok(())
}
