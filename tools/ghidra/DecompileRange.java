// Decompile every function whose entry lies in [start, end), in address order, into one file.
// For a small C++ engine DLL whose own code sits ahead of the CRT, this is the whole plugin.
// Usage: -postScript DecompileRange.java <out_file> <start> <end>

import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;

import java.io.File;
import java.io.FileWriter;
import java.io.PrintWriter;

public class DecompileRange extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File out = new File(args[0]);
        out.getParentFile().mkdirs();
        Address start = toAddr(args[1]);
        Address end = toAddr(args[2]);
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        int n = 0;
        try (PrintWriter w = new PrintWriter(new FileWriter(out))) {
            for (Function f : currentProgram.getFunctionManager().getFunctions(start, true)) {
                if (f.getEntryPoint().compareTo(end) >= 0) break;
                DecompileResults res = dec.decompileFunction(f, 600, monitor);
                String body = res.decompileCompleted() ? res.getDecompiledFunction().getC() : "// decompile failed: " + res.getErrorMessage();
                w.println("// ===== " + f.getName() + " @ " + f.getEntryPoint() + " size=" + f.getBody().getNumAddresses());
                w.println(body);
                n++;
            }
        }
        dec.dispose();
        println("decompiled " + n + " functions");
    }
}
