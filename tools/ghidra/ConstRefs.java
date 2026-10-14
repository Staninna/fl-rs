// Find initialised data words equal to the given f32 or f64 constants, list the functions
// that reference them, and decompile those functions.
// Usage: -postScript ConstRefs.java <out_dir> <value1,value2,...> [listonly]
// Each value is parsed as a double; both its f32 and f64 encodings are searched.

import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.mem.MemoryBlock;
import ghidra.program.model.symbol.Reference;

import java.io.File;
import java.io.FileWriter;
import java.io.PrintWriter;
import java.util.*;

public class ConstRefs extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File outDir = new File(args[0]);
        outDir.mkdirs();
        boolean listOnly = args.length > 2 && args[2].equals("listonly");
        Set<Function> todo = new LinkedHashSet<>();
        try (PrintWriter idx = new PrintWriter(new FileWriter(new File(outDir, "constrefs.txt"), true))) {
            for (String v : args[1].split(",")) {
                double d = Double.parseDouble(v.trim());
                byte[] f32 = le(Float.floatToIntBits((float) d), 4);
                byte[] f64 = le(Double.doubleToLongBits(d), 8);
                for (MemoryBlock blk : currentProgram.getMemory().getBlocks()) {
                    if (!blk.isInitialized()) continue;
                    for (byte[] pat : new byte[][]{f32, f64}) {
                        Address a = blk.getStart();
                        while (a != null && a.compareTo(blk.getEnd()) < 0) {
                            a = currentProgram.getMemory().findBytes(a, blk.getEnd(), pat, null, true, monitor);
                            if (a == null) break;
                            for (Reference r : getReferencesTo(a)) {
                                Function f = getFunctionContaining(r.getFromAddress());
                                if (f == null) continue;
                                idx.println(v + " @" + a + " <- " + f.getName() + "@" + f.getEntryPoint() + " at " + r.getFromAddress());
                                todo.add(f);
                            }
                            a = a.add(pat.length);
                        }
                    }
                }
            }
        }
        println("functions: " + todo.size());
        if (listOnly) return;
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        for (Function f : todo) {
            DecompileResults res = dec.decompileFunction(f, 600, monitor);
            String body = res.decompileCompleted() ? res.getDecompiledFunction().getC() : "// decompile failed: " + res.getErrorMessage();
            try (PrintWriter w = new PrintWriter(new FileWriter(new File(outDir, f.getName() + "_" + f.getEntryPoint() + ".c")))) {
                w.print(body);
            }
        }
        dec.dispose();
    }

    private static byte[] le(long v, int n) {
        byte[] b = new byte[n];
        for (int i = 0; i < n; i++) b[i] = (byte) (v >> (8 * i));
        return b;
    }
}
