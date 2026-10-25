//! Fixed-layout SPI transaction descriptors. No language List/enum layout in the BSP ABI.
use super::*;

pub(super) fn is_operation(name: &str) -> bool {
    matches!(
        name,
        "spi_create_device" | "spi_transaction" | "spi_recover"
    )
}

pub(super) fn resolve(
    flow: &emela_lir::Module,
    function: &Function,
) -> Result<Option<peripheral::Operation>, Vec<Error>> {
    let name = function
        .name
        .strip_prefix("Std.Embedded.Board.spi_")
        .unwrap();
    let effect = flow
        .core
        .effects
        .iter()
        .find(|(_, e)| e.name == "Std.Embedded.Spi.Spi")
        .map(|(_, e)| e);
    let op = effect.and_then(|e| e.ops.iter().find(|op| op.name == name));
    let params: Vec<_> = function
        .params
        .iter()
        .map(|&p| function.vars[p].ty)
        .collect();
    let valid = op.is_some_and(|op| params == op.params && matches!(&flow.core.types[function.ret], Type::Tuple(fields) if fields == &[TypeId::INT, TypeId::INT, TypeId::INT, op.ret]));
    if !valid || function.fails || function.suspends {
        return Err(unsupported(
            &function.name,
            "invalid SPI transaction binding signature",
        ));
    }
    Ok(Some(peripheral::Operation {
        peripheral: "spi".into(),
        name: name.into(),
        params,
        ret: function.ret,
        fails: false,
    }))
}

pub(super) fn header(capacity: u16, writer: &mut Writer) {
    writer.line(&format!(
        "#define EMELA_SPI_TRANSACTION_OPERATION_CAPACITY UINT16_C({capacity})"
    ));
    writer.line(&format!(
        "#define EMELA_SPI_TRANSACTION_WORD_CAPACITY UINT16_C({capacity})"
    ));
    writer.line("#define EMELA_SPI_DEVICE_CAPACITY 4");
    writer.line("enum { EMELA_SPI_OPERATION_WRITE = 1, EMELA_SPI_OPERATION_READ, EMELA_SPI_OPERATION_TRANSFER };");
    writer.line("enum { EMELA_SPI_FAILURE_NONE, EMELA_SPI_FAILURE_SPI, EMELA_SPI_FAILURE_GPIO };");
    writer
        .line("typedef struct { uint16_t kind, length, tx_offset, rx_offset; } EmelaSpiOperation;");
    writer.line(
        "typedef struct { int32_t source, status, cleanup_status; } EmelaSpiTransactionStatus;",
    );
    writer.line("void emela_spi_create_device(EmelaSpiDeviceConfig config, EmelaSpiDevice *out, EmelaSpiTransactionStatus *status);");
    writer.line("void emela_spi_transaction(EmelaSpiDevice device, const EmelaSpiOperation *operations, uint16_t count, const uint16_t *tx, uint16_t tx_length, int64_t timeout_ms, uint16_t *out, uint16_t capacity, EmelaSpiTransactionStatus *status);");
    writer.line("void emela_spi_recover(EmelaSpiId id, EmelaSpiTransactionStatus *status);");
}

pub(super) fn emit_adapter(
    flow: &emela_lir::Module,
    layouts: &TypeLayouts<'_>,
    function: &Function,
    op: &peripheral::Operation,
    variables: &[String],
    capacity: u16,
    writer: &mut Writer,
) -> Result<(), Vec<Error>> {
    writer.decl(&layouts.c_type(function.ret), "result", Some("{0}"));
    writer.line("EmelaSpiTransactionStatus status = {0};");
    let first = peripheral::to_board(layouts, op.params[0], &variables[0])?;
    match op.name.as_str() {
        "create_device" => {
            writer.line("EmelaSpiDevice device = {0};");
            writer.line(&format!(
                "emela_spi_create_device({first}, &device, &status);"
            ));
            writer.line("result.f3.payload.v0.f0 = device.slot;");
        }
        "recover" => writer.line(&format!("emela_spi_recover({first}, &status);")),
        "transaction" => {
            let Type::List(operation) = flow.core.types[op.params[1]] else {
                unreachable!()
            };
            let list = layouts.list_node_name(op.params[1]);
            let words = layouts.enum_fields(operation).expect("SPI operation")[0][0];
            let word_node = layouts.list_node_name(words);
            writer.line(&format!("EmelaSpiOperation operations[{capacity}];"));
            writer.line(&format!(
                "uint16_t tx[{capacity}], output[{capacity}] = {{0}};"
            ));
            writer.line("uint32_t count = 0, tx_count = 0, rx_count = 0;");
            writer.begin_block(&format!("for (const {list} *cursor = (const {list} *){}; cursor != NULL; cursor = (const {list} *)cursor->tail)", variables[1]));
            reject(
                writer,
                &format!("count == UINT32_C({capacity})"),
                "CAPACITY_EXCEEDED",
            );
            writer.line(&format!(
                "{} operation = cursor->head;",
                layouts.c_type(operation)
            ));
            reject(
                writer,
                "operation.tag < 0 || operation.tag > 2",
                "INVALID_ARGUMENT",
            );
            writer.line("EmelaSpiOperation *descriptor = &operations[count++];");
            writer.line("descriptor->kind = (uint16_t)(operation.tag + 1); descriptor->tx_offset = (uint16_t)tx_count; descriptor->rx_offset = (uint16_t)rx_count;");
            writer.line("uint32_t length = 0;");
            writer.begin_block("if (operation.tag == 1)");
            writer.line("int32_t size = operation.payload.v1.f0;");
            reject(writer, "size < 0", "INVALID_ARGUMENT");
            writer.line("length = (uint32_t)size;");
            writer.end_block();
            writer.begin_block("else");
            writer.begin_block(&format!("for (const {word_node} *word = (const {word_node} *)(operation.tag == 0 ? operation.payload.v0.f0 : operation.payload.v2.f0); word != NULL; word = (const {word_node} *)word->tail)"));
            reject(
                writer,
                &format!("tx_count == UINT32_C({capacity})"),
                "CAPACITY_EXCEEDED",
            );
            reject(
                writer,
                "word->head < 0 || word->head > 65535",
                "INVALID_ARGUMENT",
            );
            writer.line("tx[tx_count++] = (uint16_t)word->head; length++;");
            writer.end_block();
            writer.end_block();
            if capacity != u16::MAX {
                reject(
                    writer,
                    &format!("length > UINT32_C({capacity})"),
                    "CAPACITY_EXCEEDED",
                );
            }
            writer.begin_block("if (operation.tag != 0)");
            reject(
                writer,
                &format!("length > UINT32_C({capacity}) - rx_count"),
                "CAPACITY_EXCEEDED",
            );
            writer.line("rx_count += length;");
            writer.end_block();
            writer.line("descriptor->length = (uint16_t)length;");
            writer.end_block();
            writer.line("(void)rx_count;");
            writer.line(&format!("emela_spi_transaction({first}, count ? operations : NULL, (uint16_t)count, tx_count ? tx : NULL, (uint16_t)tx_count, {}, output, UINT16_C({capacity}), &status);", variables[2]));
            writer.begin_block("if (status.source == 0 && status.cleanup_status == 0)");
            let Type::Tuple(ref fields) = flow.core.types[function.ret] else {
                unreachable!()
            };
            let outer = fields[3];
            let Type::List(inner) = flow.core.types[outer] else {
                unreachable!()
            };
            let outer_node = layouts.list_node_name(outer);
            let inner_node = layouts.list_node_name(inner);
            let outer_kind = layouts.list_index(outer);
            let inner_kind = layouts.list_index(inner);
            writer.begin_block("for (uint32_t index = count; index-- > 0;)");
            writer.line("const EmelaSpiOperation *descriptor = &operations[index];");
            writer.line("if (descriptor->kind == EMELA_SPI_OPERATION_WRITE) continue;");
            writer.line("void *row = NULL;");
            writer.begin_block("for (uint32_t i = descriptor->length; i-- > 0;)");
            writer.line(&format!("{inner_node} *node = ({inner_node} *)emela_heap_allocate(sizeof({inner_node}), UINT32_C({inner_kind}));"));
            writer.line("node->head = (int32_t)output[descriptor->rx_offset + i]; node->tail = row; row = node;");
            writer.end_block();
            writer.line(&format!("{outer_node} *node = ({outer_node} *)emela_heap_allocate(sizeof({outer_node}), UINT32_C({outer_kind}));"));
            writer.line("node->head = row; node->tail = result.f3; result.f3 = node;");
            writer.end_block();
            writer.end_block();
        }
        _ => unreachable!(),
    }
    writer.line(
        "result.f0 = status.source; result.f1 = status.status; result.f2 = status.cleanup_status;",
    );
    writer.ret(Some("result"));
    Ok(())
}

fn reject(writer: &mut Writer, condition: &str, status: &str) {
    writer.line(&format!("if ({condition}) {{ result.f0 = EMELA_SPI_FAILURE_SPI; result.f1 = EMELA_PERIPHERAL_{status}; return result; }}"));
}
