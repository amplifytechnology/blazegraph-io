# Dependencies

This directory contains external dependencies required for the JNI backend.

## blazing-tika.jar

The JNI backend requires `blazing-tika.jar` which contains Apache Tika with custom PDF processing.

### Building the JAR

From the `tika` repo:

```bash
# Build the JAR with JNI support
mvn clean package -f pom-jni-slim.xml -DskipTests

# Copy to this directory
cp target/blazing-tika-jni-*.jar /path/to/blazegraph-core/deps/blazing-tika.jar
```

### What's in the JAR

The JAR includes:
- Apache Tika PDF parser with bounding box extraction
- Apache PDFBox for PDF rendering
- Font handling for accurate text extraction
- Custom `TikaMain.java` entry point for JNI calls

### JNI Entry Point

```java
// Called from Rust via JNI
public class TikaMain {
    public static String processToXhtml(byte[] pdfBytes) throws Exception;
}
```

## tika/

Reference Tika source files. Not used at runtime.
