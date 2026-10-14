// Write the instruction listing of the functions at the given addresses.
// Each memory operand that points at initialised data is followed by that
// data as f32 and f64, so constant-pool loads can be read without a second pass.
// Usage: -postScript DisasmAddrs.java <out_dir> <addr1,addr2,...>

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.listing.InstructionIterator;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.symbol.Reference;

import java.io.File;
import java.io.FileWriter;
import java.io.PrintWriter;

public class DisasmAddrs extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File outDir = new File(args[0]);
        outDir.mkdirs();
        Memory mem = currentProgram.getMemory();
        for (String a : args[1].split(",")) {
            Function f = getFunctionAt(toAddr(a.trim()));
            if (f == null) { println("no function at " + a); continue; }
            try (PrintWriter w = new PrintWriter(new FileWriter(new File(outDir, f.getName() + "_" + f.getEntryPoint() + ".asm")))) {
                InstructionIterator it = currentProgram.getListing().getInstructions(f.getBody(), true);
                while (it.hasNext()) {
                    Instruction ins = it.next();
                    StringBuilder sb = new StringBuilder();
                    sb.append(ins.getAddress()).append("  ").append(ins.toString());
                    for (Reference r : ins.getReferencesFrom()) {
                        Address t = r.getToAddress();
                        if (!t.isMemoryAddress() || !r.getReferenceType().isData()) continue;
                        if (!mem.contains(t) || getFunctionContaining(t) != null) continue;
                        try {
                            byte[] b = new byte[8];
                            mem.getBytes(t, b);
                            long q = 0;
                            for (int i = 7; i >= 0; i--) q = (q << 8) | (b[i] & 0xff);
                            int d = (int) q;
                            sb.append("    ; [").append(t).append("] f32=").append(Float.intBitsToFloat(d))
                              .append(" f64=").append(Double.longBitsToDouble(q))
                              .append(String.format(" u32=%08x", d));
                        } catch (Exception e) { sb.append("    ; [").append(t).append("] ?"); }
                    }
                    w.println(sb);
                }
            }
            println("disassembled " + f.getName());
        }
    }
}
