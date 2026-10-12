fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Same contract the worker and gateway compile; one file for the three.
    tonic_prost_build::compile_protos("../../proto/dfs.proto")?;
    Ok(())
}
