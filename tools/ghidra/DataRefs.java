// Decompile every function that references the given data addresses, and
// list the references. The way to the writer of a dispatch pointer or a
// table that lives in .bss, where no constant is known to search for.
// Usage: -postScript DataRefs.java <out_dir> <addr1,addr2,...>

import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;

import java.io.File;
import java.io.FileWriter;
import java.io.PrintWriter;
import java.util.*;

public class DataRefs extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File outDir = new File(args[0]);
        outDir.mkdirs();
        Set<Function> todo = new LinkedHashSet<>();
        try (PrintWriter refs = new PrintWriter(new FileWriter(new File(outDir, "refs.txt")))) {
            for (String a : args[1].split(",")) {
                Address target = toAddr(a.trim());
                for (Reference r : getReferencesTo(target)) {
                    Address from = r.getFromAddress();
                    Function f = getFunctionContaining(from);
                    String name = f == null ? "-" : f.getName();
                    refs.println(target + "\t" + from + "\t" + r.getReferenceType() + "\t" + name);
                    if (f != null) todo.add(f);
                }
            }
        }
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        for (Function f : todo) {
            DecompileResults res = dec.decompileFunction(f, 120, monitor);
            String text = res.getDecompiledFunction() == null ? "// decompile failed: " + res.getErrorMessage()
                                                               : res.getDecompiledFunction().getC();
            File out = new File(outDir, f.getName() + "_" + f.getEntryPoint() + ".c");
            try (PrintWriter w = new PrintWriter(new FileWriter(out))) {
                w.print(text);
            }
            println("decompiled " + f.getName() + " size=" + text.length());
        }
        dec.dispose();
    }
}
