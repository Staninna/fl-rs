// Decompile the functions at the given addresses, plus optionally their direct callees.
// Usage: -postScript DecompileAddrs.java <out_dir> <addr1,addr2,...> [callees]

import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.listing.Function;

import java.io.File;
import java.io.FileWriter;
import java.io.PrintWriter;
import java.util.*;

public class DecompileAddrs extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File outDir = new File(args[0]);
        outDir.mkdirs();
        boolean callees = args.length > 2 && args[2].equals("callees");
        Set<Function> todo = new LinkedHashSet<>();
        for (String a : args[1].split(",")) {
            // An address inside a function names that function, so a site
            // found by ripscan.py can be decompiled without its entry.
            Function f = getFunctionContaining(toAddr(a.trim()));
            if (f == null) { println("no function at " + a); continue; }
            todo.add(f);
            if (callees) todo.addAll(f.getCalledFunctions(monitor));
        }
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        for (Function f : todo) {
            DecompileResults res = dec.decompileFunction(f, 600, monitor);
            String body = res.decompileCompleted() ? res.getDecompiledFunction().getC() : "// decompile failed: " + res.getErrorMessage();
            try (PrintWriter w = new PrintWriter(new FileWriter(new File(outDir, f.getName() + "_" + f.getEntryPoint() + ".c")))) {
                w.print(body);
            }
            println("decompiled " + f.getName() + " size=" + f.getBody().getNumAddresses());
        }
        dec.dispose();
    }
}
