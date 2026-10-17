// Compile-time stubs for hidden framework types. Used with compileOnly, never packaged:
// at runtime the real classes come from the boot classpath.
plugins {
    `java-library`
}

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}
