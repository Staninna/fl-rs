// Decompile the functions that call the given addresses (one level up).
// Prints "callers of X: a, b, c" so the caller list survives even when a decompile fails.
// Usage: -postScript DecompileCallers.java <out_dir> <addr1,addr2,...> [listonly]

import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.listing.Function;

import java.io.File;
import java.io.FileWriter;
import java.io.PrintWriter;
import java.util.*;

public class DecompileCallers extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File outDir = new File(args[0]);
        outDir.mkdirs();
        boolean listOnly = args.length > 2 && args[2].equals("listonly");
        Set<Function> todo = new LinkedHashSet<>();
        try (PrintWriter idx = new PrintWriter(new FileWriter(new File(outDir, "callers.txt"), true))) {
            for (String a : args[1].split(",")) {
                Function f = getFunctionAt(toAddr(a.trim()));
                if (f == null) { println("no function at " + a); continue; }
                Set<Function> callers = f.getCallingFunctions(monitor);
                StringBuilder sb = new StringBuilder("callers of " + f.getName() + ":");
                for (Function c : callers) sb.append(' ').append(c.getName()).append('@').append(c.getEntryPoint());
                idx.println(sb);
                println(sb.toString());
                todo.addAll(callers);
            }
        }
        if (listOnly) return;
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
