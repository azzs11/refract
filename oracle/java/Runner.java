// Refract oracle runner for java.util.regex (protocol: oracle/protocol.md).
// Dependency-free: includes a minimal JSON reader for the request shape and a writer.
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;
import java.util.regex.*;

public class Runner {
    public static void main(String[] args) throws IOException {
        BufferedReader in = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8));
        PrintStream out = new PrintStream(new FileOutputStream(FileDescriptor.out), true, StandardCharsets.UTF_8);
        String line;
        while ((line = in.readLine()) != null) {
            if (line.isBlank()) continue;
            String resp;
            try {
                resp = handle((Map<?, ?>) new Json(line).value());
            } catch (Throwable e) {
                resp = "{\"id\":null,\"ok\":false,\"error\":" + str("runner: " + e) + "}";
            }
            out.println(resp);
        }
    }

    static String handle(Map<?, ?> req) {
        Object id = req.get("id");
        String op = (String) req.get("op");
        if ("version".equals(op)) {
            return "{\"id\":" + id + ",\"ok\":true,\"results\":[],\"engine\":\"java.util.regex\",\"version\":"
                + str(System.getProperty("java.version")) + "}";
        }
        Pattern p;
        try {
            p = Pattern.compile((String) req.get("pattern"));
        } catch (PatternSyntaxException e) {
            return "{\"id\":" + id + ",\"ok\":false,\"error\":" + str(e.getDescription()) + "}";
        } catch (IllegalArgumentException e) {
            return "{\"id\":" + id + ",\"ok\":false,\"error\":" + str(String.valueOf(e.getMessage())) + "}";
        } catch (StackOverflowError e) {
            return "{\"id\":" + id + ",\"ok\":false,\"error\":\"stack overflow while compiling\"}";
        }
        if ("time".equals(op)) {
            StringBuilder tb = new StringBuilder("{\"id\":" + id + ",\"ok\":true,\"times\":[");
            boolean full = "full".equals(req.get("mode"));
            List<?> inputs = (List<?>) req.get("inputs");
            for (int i = 0; inputs != null && i < inputs.size(); i++) {
                Matcher m = p.matcher((String) inputs.get(i));
                long t0 = System.nanoTime();
                try {
                    if (full) m.matches(); else m.find();
                } catch (StackOverflowError e) {
                    // Deep backtracking can overflow the stack: report as very slow.
                }
                tb.append(i == 0 ? "" : ",").append(System.nanoTime() - t0);
            }
            return tb.append("]}").toString();
        }
        StringBuilder sb = new StringBuilder("{\"id\":" + id + ",\"ok\":true,\"results\":[");
        if (!"compile".equals(op)) {
            List<?> inputs = (List<?>) req.get("inputs");
            for (int i = 0; inputs != null && i < inputs.size(); i++) {
                Matcher m = p.matcher((String) inputs.get(i));
                boolean r = "full".equals(op) ? m.matches() : m.find();
                sb.append(i == 0 ? "" : ",").append(r);
            }
        }
        return sb.append("]}").toString();
    }

    static String str(String s) {
        StringBuilder sb = new StringBuilder("\"");
        for (char c : s.toCharArray()) {
            if (c == '"' || c == '\\') sb.append('\\').append(c);
            else if (c < 0x20 || c > 0x7e) sb.append(String.format("\\u%04x", (int) c));
            else sb.append(c);
        }
        return sb.append('"').toString();
    }

    /** Minimal JSON reader: objects, arrays, strings (with \\u escapes), numbers, true/false/null. */
    static final class Json {
        final String s; int i;
        Json(String s) { this.s = s; }
        Object value() {
            ws();
            char c = s.charAt(i);
            if (c == '{') { i++; Map<String, Object> m = new HashMap<>(); ws();
                if (s.charAt(i) == '}') { i++; return m; }
                while (true) { ws(); String k = (String) value(); ws(); i++; m.put(k, value()); ws();
                    if (s.charAt(i++) == '}') return m; } }
            if (c == '[') { i++; List<Object> l = new ArrayList<>(); ws();
                if (s.charAt(i) == ']') { i++; return l; }
                while (true) { l.add(value()); ws(); if (s.charAt(i++) == ']') return l; } }
            if (c == '"') { i++; StringBuilder sb = new StringBuilder();
                while (true) { char d = s.charAt(i++);
                    if (d == '"') return sb.toString();
                    if (d != '\\') { sb.append(d); continue; }
                    char e = s.charAt(i++);
                    switch (e) {
                        case 'n' -> sb.append('\n'); case 't' -> sb.append('\t'); case 'r' -> sb.append('\r');
                        case 'b' -> sb.append('\b'); case 'f' -> sb.append('\f');
                        case 'u' -> { sb.append((char) Integer.parseInt(s.substring(i, i + 4), 16)); i += 4; }
                        default -> sb.append(e);
                    } } }
            int j = i;
            while (i < s.length() && ",}] ".indexOf(s.charAt(i)) < 0) i++;
            String t = s.substring(j, i);
            return switch (t) { case "true" -> true; case "false" -> false; case "null" -> null; default -> Long.parseLong(t); };
        }
        void ws() { while (i < s.length() && Character.isWhitespace(s.charAt(i))) i++; }
    }
}
