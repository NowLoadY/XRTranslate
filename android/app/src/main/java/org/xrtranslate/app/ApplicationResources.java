package org.xrtranslate.app;

import android.content.Context;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;

/** The UI and background translator prepare the same installed resources. */
public final class ApplicationResources {
    private ApplicationResources() {}

    public static synchronized void prepare(Context context, boolean all) throws IOException {
        copy(context, all ? "application" : "application/config.json",
            all ? context.getFilesDir() : new File(context.getFilesDir(), "config.json"));
    }

    private static void copy(Context context, String source, File destination) throws IOException {
        String[] children = context.getAssets().list(source);
        if (children != null && children.length > 0) {
            if (!destination.isDirectory() && !destination.mkdirs()) throw new IOException("Cannot create application storage");
            for (String child : children) copy(context, source + "/" + child, new File(destination, child));
        } else if (!destination.exists()) {
            File temporary = File.createTempFile("resource-", ".part", destination.getParentFile());
            try {
                try (InputStream input = context.getAssets().open(source); FileOutputStream output = new FileOutputStream(temporary)) {
                    byte[] buffer = new byte[65536];
                    for (int count; (count = input.read(buffer)) >= 0;) {
                        if (Thread.currentThread().isInterrupted()) throw new IOException("Resource preparation cancelled");
                        output.write(buffer, 0, count);
                    }
                    output.getFD().sync();
                }
                if (!temporary.renameTo(destination)) throw new IOException("Cannot prepare application resource");
            } finally { temporary.delete(); }
        }
    }
}
