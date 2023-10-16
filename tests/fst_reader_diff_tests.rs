// Copyright 2023 The Regents of the University of California
// released under BSD 3-Clause License
// author: Kevin Laeufer <laeufer@berkeley.edu>

use fst_native::*;
use std::collections::VecDeque;
use std::ffi::{c_char, c_uchar, c_void, CStr, CString};
use std::fs::File;
use std::future::pending;

fn fst_sys_load_header(handle: *mut c_void) -> FstHeader {
    unsafe {
        let version = fst_sys::fstReaderGetVersionString(handle);
        let date = fst_sys::fstReaderGetDateString(handle);
        FstHeader {
            start_time: fst_sys::fstReaderGetStartTime(handle),
            end_time: fst_sys::fstReaderGetEndTime(handle),
            var_count: fst_sys::fstReaderGetVarCount(handle),
            max_handle: fst_sys::fstReaderGetMaxHandle(handle) as u64,
            version: CStr::from_ptr(version).to_str().unwrap().to_string(),
            date: CStr::from_ptr(date).to_str().unwrap().to_string(),
        }
    }
}

fn fst_sys_load_hierarchy(handle: *mut c_void) -> VecDeque<String> {
    let mut out = VecDeque::new();
    unsafe { fst_sys::fstReaderIterateHierRewind(handle) };
    loop {
        let p = unsafe {
            let ptr = fst_sys::fstReaderIterateHier(handle);
            if ptr.is_null() {
                None
            } else {
                Some(&*ptr)
            }
        };
        if p.is_none() {
            break;
        }
        let value = p.unwrap();
        out.push_back(fst_sys_hierarchy_to_str(value));
    }
    out
}

unsafe fn fst_sys_hierarchy_read_name(ptr: *const c_char, len: u32) -> String {
    let slic = std::slice::from_raw_parts(ptr as *const u8, len as usize);
    (std::str::from_utf8(slic)).unwrap().to_string()
}

fn fst_sys_scope_tpe_to_string(tpe: fst_sys::fstScopeType) -> String {
    let con = match tpe {
        fst_sys::fstScopeType_FST_ST_VCD_MODULE => "Module",
        fst_sys::fstScopeType_FST_ST_VCD_TASK => "Task",
        fst_sys::fstScopeType_FST_ST_VCD_FUNCTION => "Function",
        fst_sys::fstScopeType_FST_ST_VCD_BEGIN => "Begin",
        other => todo!("scope type: {other}"),
    };
    con.to_string()
}

unsafe fn fst_sys_parse_attribute(attr: &fst_sys::fstHier__bindgen_ty_1_fstHierAttr) -> String {
    let name = fst_sys_hierarchy_read_name(attr.name, attr.name_length);
    match attr.typ as fst_sys::fstAttrType {
        fst_sys::fstAttrType_FST_AT_MISC => {
            let misc_tpe = attr.subtype as fst_sys::fstMiscType;
            match misc_tpe {
                fst_sys::fstMiscType_FST_MT_PATHNAME => {
                    let id = attr.arg;
                    format!("PathName: {id} -> {name}")
                }
                fst_sys::fstMiscType_FST_MT_SOURCEISTEM
                | fst_sys::fstMiscType_FST_MT_SOURCESTEM => {
                    let line = attr.arg;
                    let path_id = leb128::read::unsigned(&mut name.as_bytes()).unwrap();
                    let is_instantiation = misc_tpe == fst_sys::fstMiscType_FST_MT_SOURCEISTEM;
                    format!("SourceStem:: {is_instantiation}, {path_id}, {line}")
                }
                7 => {
                    // FST_MT_ENUMTABLE (missing from fst_sys)
                    if name.is_empty() {
                        format!("EnumTableRef: {}", attr.arg)
                    } else {
                        format!("EnumTable: {name} ({})", attr.arg)
                    }
                }
                fst_sys::fstMiscType_FST_MT_COMMENT => {
                    format!("Comment: {name}")
                }
                other => todo!("misc attribute of subtype {other}"),
            }
        }
        _ => format!("BeginAttr: {name}"),
    }
}

fn fst_sys_hierarchy_to_str(entry: &fst_sys::fstHier) -> String {
    match entry.htyp as u32 {
        fst_sys::fstHierType_FST_HT_SCOPE => {
            let name = unsafe {
                fst_sys_hierarchy_read_name(entry.u.scope.name, entry.u.scope.name_length)
            };
            let component = unsafe {
                fst_sys_hierarchy_read_name(entry.u.scope.component, entry.u.scope.component_length)
            };
            let tpe =
                unsafe { fst_sys_scope_tpe_to_string(entry.u.scope.typ as fst_sys::fstScopeType) };
            format!("Scope: {name} ({tpe}) {component}")
        }
        fst_sys::fstHierType_FST_HT_UPSCOPE => "UpScope".to_string(),
        fst_sys::fstHierType_FST_HT_VAR => {
            let handle = unsafe { entry.u.var.handle };
            let name =
                unsafe { fst_sys_hierarchy_read_name(entry.u.var.name, entry.u.var.name_length) };
            format!("(H{handle}): {name}")
        }
        fst_sys::fstHierType_FST_HT_ATTRBEGIN => unsafe { fst_sys_parse_attribute(&entry.u.attr) },
        fst_sys::fstHierType_FST_HT_ATTREND => "EndAttr".to_string(),
        other => todo!("htype={other}"),
    }
}

fn hierarchy_to_str(entry: &FstHierarchyEntry) -> String {
    match entry {
        FstHierarchyEntry::Scope {
            name,
            tpe,
            component,
        } => format!("Scope: {name} ({}) {component}", hierarchy_tpe_to_str(tpe)),
        FstHierarchyEntry::UpScope => "UpScope".to_string(),
        FstHierarchyEntry::Var { name, handle, .. } => format!("({handle}): {name}"),
        FstHierarchyEntry::AttributeBegin { name } => format!("BeginAttr: {name}"),
        FstHierarchyEntry::AttributeEnd => "EndAttr".to_string(),
        FstHierarchyEntry::PathName { name, id } => format!("PathName: {id} -> {name}"),
        FstHierarchyEntry::SourceStem {
            is_instantiation,
            path_id,
            line,
        } => format!("SourceStem:: {is_instantiation}, {path_id}, {line}"),
        FstHierarchyEntry::Comment { string } => format!("Comment: {string}"),
        FstHierarchyEntry::EnumTable {
            name,
            handle,
            mapping,
        } => {
            let names = mapping
                .iter()
                .map(|(v, n)| n.clone())
                .collect::<Vec<_>>()
                .join(" ");
            let values = mapping
                .iter()
                .map(|(v, n)| v.clone())
                .collect::<Vec<_>>()
                .join(" ");
            format!(
                "EnumTable: {name} {} {names} {values} ({handle})",
                mapping.len()
            )
        }
        FstHierarchyEntry::EnumTableRef { handle } => format!("EnumTableRef: {handle}"),
    }
}

fn hierarchy_tpe_to_str(tpe: &FstScopeType) -> String {
    let con = match tpe {
        FstScopeType::Module => "Module",
        FstScopeType::Task => "Task",
        FstScopeType::Function => "Function",
        FstScopeType::Begin => "Begin",
        FstScopeType::Fork => "Fork",
        FstScopeType::Generate => "Generate",
        FstScopeType::Struct => "Struct",
        FstScopeType::Union => "Union",
        FstScopeType::Class => "Class",
        FstScopeType::Interface => "Interface",
        FstScopeType::Package => "Package",
        FstScopeType::Program => "Program",
        FstScopeType::VhdlArchitecture => "VhdlArchitecture",
        FstScopeType::VhdlProcedure => "VhdlProcedure",
        FstScopeType::VhdlFunction => "VhdlFunction",
        FstScopeType::VhdlRecord => "VhdlRecord",
        FstScopeType::VhdlProcess => "VhdlProcess",
        FstScopeType::VhdlBlock => "VhdlBlock",
        FstScopeType::VhdlForGenerate => "VhdlForGenerate",
        FstScopeType::VhdlIfGenerate => "VhdlIfGenerate",
        FstScopeType::VhdlGenerate => "VhdlGenerate",
        FstScopeType::VhdlPackage => "VhdlPackage",
        FstScopeType::AttributeBegin => "AttributeBegin",
        FstScopeType::AttributeEnd => "AttributeEnd",
        FstScopeType::VcdScope => "VcdScope",
        FstScopeType::VcdUpScope => "VcdUpScope",
    };
    con.to_string()
}

fn diff_hierarchy<R: std::io::Read + std::io::Seek>(
    our_reader: &mut FstReader<R>,
    mut exp_hierarchy: VecDeque<String>,
) -> Vec<bool> {
    let mut is_real = Vec::new();
    let check = |entry: FstHierarchyEntry| {
        // remember if variables are real valued
        match &entry {
            FstHierarchyEntry::Var { tpe, handle, .. } => {
                let is_var_real = match tpe {
                    FstVarType::Real
                    | FstVarType::RealParameter
                    | FstVarType::RealTime
                    | FstVarType::ShortReal => true,
                    _ => false,
                };
                let idx = handle.get_index();
                if is_real.len() <= idx {
                    is_real.resize(idx + 1, false);
                }
                is_real[idx] = is_var_real;
            }
            _ => {}
        };

        let expected = exp_hierarchy.pop_front().unwrap();
        let actual = hierarchy_to_str(&entry);
        assert_eq!(actual, expected);
        // println!("{actual:?}");
    };
    our_reader.read_hierarchy(check).unwrap();
    is_real
}

fn fst_sys_load_signals(handle: *mut c_void, is_real: &[bool]) -> VecDeque<(u64, u32, String)> {
    let mut out = VecDeque::new();
    let mut data = CallbackData {
        out,
        is_real: Vec::from(is_real),
    };
    unsafe {
        fst_sys::fstReaderIterBlocksSetNativeDoublesOnCallback(handle, 1);
        fst_sys::fstReaderSetFacProcessMaskAll(handle);
        let data_ptr = (&mut data) as *mut CallbackData;
        let user_ptr = data_ptr as *mut c_void;
        fst_sys::fstReaderIterBlocks2(
            handle,
            Some(signal_change_callback),
            Some(var_signal_change_callback),
            user_ptr,
            std::ptr::null_mut(),
        );
    }
    data.out
}

struct CallbackData {
    out: VecDeque<(u64, u32, String)>,
    is_real: Vec<bool>,
}

extern "C" fn signal_change_callback(
    data_ptr: *mut c_void,
    time: u64,
    handle: fst_sys::fstHandle,
    value: *const c_uchar,
) {
    let data = unsafe { &mut *(data_ptr as *mut CallbackData) };
    let signal_idx = (handle - 1) as usize;
    let string = if data.is_real[signal_idx] {
        let slic = unsafe { std::slice::from_raw_parts(value as *const u8, 8) };
        let value = f64::from_le_bytes(slic.try_into().unwrap());
        format!("{value}")
    } else {
        unsafe {
            CStr::from_ptr(value as *const c_char)
                .to_str()
                .unwrap()
                .to_string()
        }
    };
    let signal = (time, handle, string);

    data.out.push_back(signal);
}

extern "C" fn var_signal_change_callback(
    data_ptr: *mut c_void,
    time: u64,
    handle: fst_sys::fstHandle,
    value: *const c_uchar,
    len: u32,
) {
    let bytes = unsafe { std::slice::from_raw_parts(value, len as usize) };
    let string: String = std::str::from_utf8(bytes).unwrap().to_string();
    let signal = (time, handle, string);
    let data = unsafe { &mut *(data_ptr as *mut CallbackData) };
    let signal_idx = (handle - 1) as usize;
    assert!(
        !data.is_real[signal_idx],
        "reals should never be variable length!"
    );
    data.out.push_back(signal);
}

fn diff_signals<R: std::io::Read + std::io::Seek>(
    our_reader: &mut FstReader<R>,
    mut exp_signals: VecDeque<(u64, u32, String)>,
) {
    let check = |time: u64, handle: FstSignalHandle, value: FstSignalValue| {
        let (exp_time, exp_handle, exp_value) = exp_signals.pop_front().unwrap();
        let actual_as_string = match value {
            FstSignalValue::String(str) => str.to_string(),
            FstSignalValue::Real(value) => format!("{value}"),
        };
        let actual = (time, handle.get_index() + 1, actual_as_string);
        let expected = (exp_time, exp_handle as usize, exp_value);
        assert_eq!(actual, expected);
        // println!("{actual:?}");
    };
    let filter = FstFilter::all();
    our_reader.read_signals(&filter, check).unwrap();
}

fn run_diff_test(filename: &str, filter: &FstFilter) {
    // open file with FST library from GTKWave
    let c_path = CString::new(filename).unwrap();
    let exp_handle = unsafe { fst_sys::fstReaderOpen(c_path.as_ptr()) };

    // open file with our library
    let our_f = File::open(filename).unwrap_or_else(|_| panic!("Failed to open {}", filename));
    let mut our_reader = FstReader::open(our_f).unwrap();

    // compare header
    let exp_header = fst_sys_load_header(exp_handle);
    let our_header = our_reader.get_header();
    assert_eq!(our_header, exp_header);

    // compare hierarchy
    let exp_hierarchy = fst_sys_load_hierarchy(exp_handle);
    let is_real = diff_hierarchy(&mut our_reader, exp_hierarchy);

    // compare signals
    let exp_signals = fst_sys_load_signals(exp_handle, &is_real);
    diff_signals(&mut our_reader, exp_signals);

    // close C-library handle
    unsafe { fst_sys::fstReaderClose(exp_handle) };
}

#[test]
fn diff_aldec_spi_write() {
    run_diff_test("fsts/aldec/SPI_Write.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_amaranth_up_counter() {
    run_diff_test("fsts/amaranth/up_counter.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_ghdl_alu() {
    run_diff_test("fsts/ghdl/alu.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_ghdl_idea() {
    run_diff_test("fsts/ghdl/idea.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_ghdl_pcpu() {
    run_diff_test("fsts/ghdl/pcpu.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_gtkwave_des() {
    run_diff_test("fsts/gtkwave-analyzer/des.fst", &FstFilter::all());
}

#[test]
fn diff_gtkwave_perm_current() {
    run_diff_test(
        "fsts/gtkwave-analyzer/perm_current.vcd.fst",
        &FstFilter::all(),
    );
}

#[test]
fn diff_gtkwave_transaction() {
    run_diff_test("fsts/gtkwave-analyzer/transaction.fst", &FstFilter::all());
}

#[test]
fn diff_icarus_cpu() {
    run_diff_test("fsts/icarus/CPU.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_icarus_rv32_soc_tb() {
    run_diff_test("fsts/icarus/rv32_soc_TB.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_icarus_test1() {
    run_diff_test("fsts/icarus/test1.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_model_sim_clkdiv2n_tb() {
    run_diff_test("fsts/model-sim/clkdiv2n_tb.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_model_sim_cpu_design() {
    run_diff_test("fsts/model-sim/CPU_Design.msim.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_my_hdl_sigmoid_tb() {
    run_diff_test("fsts/my-hdl/sigmoid_tb.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_my_hdl_simple_memory() {
    run_diff_test("fsts/my-hdl/Simple_Memory.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_my_hdl_top() {
    run_diff_test("fsts/my-hdl/top.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_ncsim_ffdiv() {
    run_diff_test("fsts/ncsim/ffdiv_32bit_tb.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_quartus_mips_hardware() {
    run_diff_test("fsts/quartus/mipsHardware.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_quartus_wave() {
    run_diff_test("fsts/quartus/wave_registradores.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_questa_sim_dump() {
    run_diff_test("fsts/questa-sim/dump.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_questa_sim_test() {
    run_diff_test("fsts/questa-sim/test.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_riviera_pro_dump() {
    run_diff_test("fsts/riviera-pro/dump.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_systemc_waveform() {
    run_diff_test("fsts/systemc/waveform.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_treadle_gcd() {
    run_diff_test("fsts/treadle/GCD.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_vcs_apb_uvm_new() {
    run_diff_test("fsts/vcs/Apb_slave_uvm_new.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_vcs_datapath_log() {
    run_diff_test("fsts/vcs/datapath_log.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_vcs_processor() {
    run_diff_test("fsts/vcs/processor.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_verilator_basic_test() {
    run_diff_test("fsts/verilator/basic_test.fst", &FstFilter::all());
}

#[test]
fn diff_verilator_many_sv_data_types() {
    run_diff_test("fsts/verilator/many_sv_datatypes.fst", &FstFilter::all());
}

#[test]
fn diff_verilator_swerv1() {
    run_diff_test("fsts/verilator/swerv1.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_verilator_vlt_dump() {
    run_diff_test("fsts/verilator/vlt_dump.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_vivado_iladata() {
    run_diff_test("fsts/vivado/iladata.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_xilinx_isim_test() {
    run_diff_test("fsts/xilinx_isim/test.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_xilinx_isim_test1() {
    run_diff_test("fsts/xilinx_isim/test1.vcd.fst", &FstFilter::all());
}

#[test]
#[ignore] // TODO: implement blackout
fn diff_xilinx_isim_test2x2_regex22_string1() {
    run_diff_test(
        "fsts/xilinx_isim/test2x2_regex22_string1.vcd.fst",
        &FstFilter::all(),
    );
}

#[test]
fn diff_scope_with_comment() {
    run_diff_test("fsts/scope_with_comment.vcd.fst", &FstFilter::all());
}

#[test]
fn diff_vcd_file_with_errors() {
    run_diff_test("fsts/VCD_file_with_errors.vcd.fst", &FstFilter::all());
}
