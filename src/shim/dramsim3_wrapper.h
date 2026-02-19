#ifndef DRAMSIM3_WRAPPER_H
#define DRAMSIM3_WRAPPER_H

#include <cstdint>
#include <string>
#include <unordered_set>
#include <utility>

namespace dramsim3 {

class MemorySystem;

struct pair_hash {
    inline std::size_t operator()(const std::pair<uint64_t, bool> &v) const {
        return v.first * 31 + (v.second ? 1 : 0);
    }
};

class DRAMSim3Wrapper {
public:
    DRAMSim3Wrapper(const std::string &config_file, const std::string &output_dir);
    ~DRAMSim3Wrapper();

    void AddTransaction(uint64_t addr, bool is_write);
    bool WillAcceptTransaction(uint64_t addr, bool is_write);
    void ClockTick();
    bool IsTransactionDone(uint64_t addr, bool is_write);

private:
    void ReadComplete(uint64_t addr);
    void WriteComplete(uint64_t addr);

    MemorySystem *memory_system_;
    std::unordered_set<std::pair<uint64_t, bool>, pair_hash> completed_transactions_;
};

} // namespace dramsim3

typedef struct CDRAMSim3 CDRAMSim3;

extern "C" {
    CDRAMSim3* new_dramsim3_wrapper(const char* config_file, const char* output_dir);
    void delete_dramsim3_wrapper(CDRAMSim3* wrapper);
    void dramsim3_add_transaction(CDRAMSim3* wrapper, uint64_t addr, bool is_write);
    bool dramsim3_will_accept_transaction(CDRAMSim3* wrapper, uint64_t addr, bool is_write);
    void dramsim3_clock_tick(CDRAMSim3* wrapper);
    bool dramsim3_is_transaction_done(CDRAMSim3* wrapper, uint64_t addr, bool is_write);
}

#endif // DRAMSIM3_WRAPPER_H
