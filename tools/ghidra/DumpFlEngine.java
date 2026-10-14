// Dump what the .flp loader needs: every string with its referrers, and the
// decompiled body of every function that references a given list of strings.
// Usage (headless):
//   analyzeHeadless <proj_dir> <proj> -import FLEngine_x64.dll -postScript DumpFlEngine.java <out_dir> <needle1,needle2,...>
// Re-runs on an existing project: -process FLEngine_x64.dll -noanalysis -postScript ...

import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Data;
import ghidra.program.model.listing.DataIterator;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;
import ghidra.program.util.DefinedStringIterator;

import java.io.File;
import java.io.FileWriter;
import java.io.PrintWriter;
import java.util.*;

public class DumpFlEngine extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        File outDir = new File(args[0]);
        outDir.mkdirs();
        Set<String> needles = new HashSet<>(Arrays.asList(args.length > 1 ? args[1].split(",") : new String[0]));

        Map<Function, Set<String>> hits = new LinkedHashMap<>();
        try (PrintWriter strings = new PrintWriter(new FileWriter(new File(outDir, "strings.tsv")))) {
            strings.println("address\tstring\treferrers");
            DataIterator it = DefinedStringIterator.forProgram(currentProgram);
            while (it.hasNext() && !monitor.isCancelled()) {
                Data d = it.next();
                Object v = d.getValue();
                if (v == null) continue;
                String s = v.toString();
                StringBuilder refs = new StringBuilder();
                for (Reference r : getReferencesTo(d.getAddress())) {
                    Function f = getFunctionContaining(r.getFromAddress());
                    if (f == null) continue;
                    if (refs.length() > 0) refs.append(',');
                    refs.append(f.getName()).append('@').append(f.getEntryPoint());
                    if (needles.contains(s)) hits.computeIfAbsent(f, k -> new TreeSet<>()).add(s);
                }
                strings.println(d.getAddress() + "\t" + s.replace('\t', ' ').replace('\n', ' ') + "\t" + refs);
            }
        }

        try (PrintWriter funcs = new PrintWriter(new FileWriter(new File(outDir, "functions.tsv")))) {
            funcs.println("entry\tname\tsize\tcallers\tcallees");
            for (Function f : currentProgram.getFunctionManager().getFunctions(true)) {
                funcs.println(f.getEntryPoint() + "\t" + f.getName() + "\t" + f.getBody().getNumAddresses()
                        + "\t" + f.getCallingFunctions(monitor).size() + "\t" + f.getCalledFunctions(monitor).size());
            }
        }

        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        File decDir = new File(outDir, "decompiled");
        decDir.mkdirs();
        for (Map.Entry<Function, Set<String>> e : hits.entrySet()) {
            Function f = e.getKey();
            DecompileResults res = dec.decompileFunction(f, 600, monitor);
            String body = res.decompileCompleted() ? res.getDecompiledFunction().getC() : "// decompile failed: " + res.getErrorMessage();
            try (PrintWriter w = new PrintWriter(new FileWriter(new File(decDir, f.getName() + "_" + f.getEntryPoint() + ".c")))) {
                w.println("// references: " + e.getValue());
                w.print(body);
            }
            println("decompiled " + f.getName() + " @ " + f.getEntryPoint() + " for " + e.getValue());
        }
        dec.dispose();
    }
}
