#include "dramsim3_wrapper.h"
#include "dramsim3.h"
#include <iostream>

namespace dramsim3 {

DRAMSim3Wrapper::DRAMSim3Wrapper(const std::string &config_file, const std::string &output_dir) {
    memory_system_ = GetMemorySystem(config_file, output_dir,
        [this](uint64_t addr) { this->ReadComplete(addr); },
        [this](uint64_t addr) { this->WriteComplete(addr); }
    );
}

DRAMSim3Wrapper::~DRAMSim3Wrapper() {
    delete memory_system_;
}

void DRAMSim3Wrapper::AddTransaction(uint64_t addr, bool is_write) {
    memory_system_->AddTransaction(addr, is_write);
}

bool DRAMSim3Wrapper::WillAcceptTransaction(uint64_t addr, bool is_write) {
    return memory_system_->WillAcceptTransaction(addr, is_write);
}

void DRAMSim3Wrapper::ClockTick() {
    // We clear completed transactions at the beginning of each tick.
    // The assumption is that MemorySystem::ClockTick() will eventually trigger
    // ReadComplete/WriteComplete callbacks within the SAME call stack if a transaction
    // finishes in this tick.
    // This allows the Rust side to check IsTransactionDone() immediately after ClockTick().
    completed_transactions_.clear();
    memory_system_->ClockTick();
}

bool DRAMSim3Wrapper::IsTransactionDone(uint64_t addr, bool is_write) {
    return completed_transactions_.count({addr, is_write}) > 0;
}

void DRAMSim3Wrapper::ReadComplete(uint64_t addr) {
    completed_transactions_.insert({addr, false});
}

void DRAMSim3Wrapper::WriteComplete(uint64_t addr) {
    completed_transactions_.insert({addr, true});
}

} // namespace dramsim3

using namespace dramsim3;

extern "C" {

CDRAMSim3* new_dramsim3_wrapper(const char* config_file, const char* output_dir) {
    return (CDRAMSim3*) new DRAMSim3Wrapper(std::string(config_file), std::string(output_dir));
}

void delete_dramsim3_wrapper(CDRAMSim3* wrapper) {
    delete (DRAMSim3Wrapper*)wrapper;
}

void dramsim3_add_transaction(CDRAMSim3* wrapper, uint64_t addr, bool is_write) {
    ((DRAMSim3Wrapper*)wrapper)->AddTransaction(addr, is_write);
}

bool dramsim3_will_accept_transaction(CDRAMSim3* wrapper, uint64_t addr, bool is_write) {
    return ((DRAMSim3Wrapper*)wrapper)->WillAcceptTransaction(addr, is_write);
}

void dramsim3_clock_tick(CDRAMSim3* wrapper) {
    ((DRAMSim3Wrapper*)wrapper)->ClockTick();
}

bool dramsim3_is_transaction_done(CDRAMSim3* wrapper, uint64_t addr, bool is_write) {
    return ((DRAMSim3Wrapper*)wrapper)->IsTransactionDone(addr, is_write);
}

}
