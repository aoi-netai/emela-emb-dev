#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include "emela_spi.h"
#include "emela_embedded.h"
#include "emela_board.h"
static uint64_t now, exchange_cost;
static unsigned exchanges, selections, deassertions, configurations;
static int32_t exchange_error, abort_error, select_error, cleanup_error, init_error;
static int32_t selected = -1;
static bool owned[8];
static bool active_levels[8], transferring;
static uint64_t active_start;
static uint64_t last_end;
static uint32_t spi_prescaler_power(EmelaSpiPrescaler p);
#define SPI_REQUIRE_MAIN() ((void)0)
static uint64_t spi_hw_now_us(void) { return now++; }
static int32_t spi_hw_validate(EmelaSpiConfig c) { return c.mode || c.direction ? EMELA_PERIPHERAL_UNSUPPORTED : 0; }
static int32_t spi_hw_configure(EmelaSpiConfig c, uint64_t s, int64_t t) { (void)s; (void)t; assert(spi_prescaler_power(c.prescaler) <= 7); assert(selected < 0); configurations++; return 0; }
static int32_t spi_hw_exchange(EmelaSpiConfig c, const uint16_t *tx, uint16_t size, uint16_t *out, uint64_t s, int64_t t) {
    assert(t > 0 && now >= s);
    if (selected >= 0) assert(now - active_start >= 3);
    now += exchange_cost; exchanges++;
    if (selected >= 0) { transferring = true; last_end = now; }
    if (out) for (uint32_t i = 0; i < size; i++) out[i] = tx ? tx[i] ^ (c.data_size ? 65535u : 255u) : 165u;
    return exchange_error;
}
static int32_t spi_hw_abort(void) { transferring = false; return abort_error; }
static int32_t spi_hw_cs_validate(int32_t pin) { return pin < 0 || pin >= 8 ? EMELA_PERIPHERAL_INVALID_ARGUMENT : owned[pin] ? EMELA_PERIPHERAL_RESOURCE_CONFLICT : 0; }
static int32_t spi_hw_cs_init(int32_t pin, int32_t inactive) { active_levels[pin] = !inactive; owned[pin] = true; return init_error; }
static int32_t spi_hw_cs_write(int32_t pin, int32_t state) {
    assert(owned[pin]);
    if (state == active_levels[pin]) { assert(selected < 0); selected = pin; active_start = now; selections++; return select_error; }
    if (transferring) assert(now - last_end >= 5);
    transferring = false;
    deassertions++; if (!cleanup_error) selected = -1;
    return cleanup_error;
}
static void spi_hw_cs_release(int32_t pin) { owned[pin] = false; }
/* @DRIVER@ */
_Noreturn void emela_board_defect(EmelaDefectKind k, const char *m, uint16_t n) { (void)k; (void)m; (void)n; abort(); }
size_t fixture_heap_live(void);
int main(void) {
    assert(transaction(0,2,0) == 101);
    assert(create(2,8) == 0 && create(3,8) == 1);
    assert(create(2,8) == 207 && create(-1,8) == 201);
    assert(transaction(0,0,1) == 42 && transaction(1,0,1) == 42);
    assert(exchanges == 6 && selections == 2 && deassertions == 2 && selected < 0);
    unsigned before = selections, transfers_before = exchanges;
    assert(transaction(0,1,0) == 43 && transaction(0,2,0) == 0);
    assert(transaction(0,3,256) == 101 && transaction(0,3,-1) == 101);
    assert(transaction(0,4,-1) == 101 && transaction(0,4,5) == 102);
    assert(transaction(0,5,0) == 102 && transaction(0,6,0) == 102 && transaction(0,7,0) == 102);
    assert(transaction(0,4,-99) == 101);
    assert(transaction(99,2,0) == 101);
    assert(selections == before && exchanges == transfers_before && fixture_heap_live() == 0);
    assert(direct(0,255) == 0 && direct(0,256) == 101 && direct(3,65535) == 1);
    assert(direct(1,4) == 4 && direct(1,5) == 102);
    assert(classify(-16) == 16 && classify(-12) == 12 && classify(-13) == 9 && classify(99) == 9 && classify(0) == 0);
    exchange_error = EMELA_PERIPHERAL_TIMEOUT; cleanup_error = EMELA_PERIPHERAL_INVALID_STATE;
    assert(transaction(0,0,1) == 1113);
    assert(transaction(1,0,1) == 108 && direct(0,1) == 108); /* configure cannot clear recovery */
    cleanup_error = 0; exchange_error = 0;
    assert(recover() == 0 && selected < 0 && transaction(1,0,1) == 42);
    select_error = EMELA_PERIPHERAL_RESOURCE_CONFLICT;
    assert(transaction(0,0,1) == 207 && selected < 0); select_error = 0;
    cleanup_error = EMELA_PERIPHERAL_OTHER;
    assert(transaction(0,0,1) == 1009); cleanup_error = 0;
    assert(recover() == 0);
    exchange_cost = 60000;
    assert(transaction(0,0,1) == 105 && selected < 0); /* one deadline, not 100ms per operation */
    exchange_cost = 0;
    assert(transaction(0,0,1) == 42);
    exchange_error = EMELA_PERIPHERAL_MODE_FAULT;
    assert(direct(0,1) == 116); exchange_error = 0;
    assert(direct(0,1) == 0);
    init_error = EMELA_PERIPHERAL_OTHER;
    assert(create(4,8) == 209); init_error = 0;
    assert(recover() == 0 && !owned[4]);
    EmelaSpiDevice high;
    EmelaSpiConfig cfg = {0,0,0,0,0,0,0};
    EmelaSpiDeviceConfig dc = {1,cfg,{4},1,3,5}; EmelaSpiTransactionStatus status;
    emela_spi_create_device(dc, &high, &status); assert(!status.source && high.slot == 2);
    uint16_t byte = 1; EmelaSpiOperation write = {1,1,0,0};
    emela_spi_transaction(high, &write, 1, &byte, 1, 100, NULL, 0, &status);
    assert(!status.source && !status.cleanup_status && selected < 0 && active_levels[4]);
    assert(create(5,8) == 3 && create(6,8) == 102);
    assert(fixture_heap_live() == 0);
}
