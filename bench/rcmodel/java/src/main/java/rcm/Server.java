package rcm;

import com.sun.net.httpserver.HttpServer;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.Executors;

/** W5 Java 25: JDK built-in HttpServer with a virtual-thread-per-request executor. Fixed JSON body. */
public final class Server {
    public static void main(String[] args) throws Exception {
        int port = args.length > 0 ? Integer.parseInt(args[0]) : 8080;
        byte[] body = "{\"status\":\"ok\",\"service\":\"rcm\",\"n\":42}".getBytes(StandardCharsets.UTF_8);
        HttpServer s = HttpServer.create(new InetSocketAddress(InetAddress.getLoopbackAddress(), port), 8192);
        s.createContext("/", ex -> {
            ex.getResponseHeaders().add("Content-Type", "application/json");
            ex.sendResponseHeaders(200, body.length);
            try (var os = ex.getResponseBody()) { os.write(body); }
        });
        s.setExecutor(Executors.newVirtualThreadPerTaskExecutor());
        s.start();
    }
}
