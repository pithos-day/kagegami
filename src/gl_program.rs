//! シェーダーをコンパイルしてリンクする共通の処理。
//!
//! 図形の描画（render.rs）、シェーダーの壁紙（shader.rs）、計測（bench.rs）で使う。
//! エラーメッセージはそれぞれ違うので、エラーは種類と GL のログだけを返し、文言は呼び出し側で作る。

use glow::HasContext;

#[derive(Debug)]
pub enum ProgramError {
    /// GL のオブジェクトを作れなかった。
    Create(String),
    /// コンパイルに失敗した。`shader_type` は `glow::VERTEX_SHADER` など。
    Compile { shader_type: u32, log: String },
    /// リンクに失敗した。
    Link(String),
}

/// `shaders` を順にコンパイルしてプログラムにつなぎ、`before_link` を呼んでからリンクする。
/// `before_link` は `bind_attrib_location` のように、リンクの前に要る設定に使う。
/// 失敗したら、作ったシェーダーとプログラムを消してからエラーを返す。
pub fn build(
    gl: &glow::Context,
    shaders: &[(u32, &str)],
    before_link: impl FnOnce(glow::Program),
) -> Result<glow::Program, ProgramError> {
    unsafe {
        let program = gl.create_program().map_err(ProgramError::Create)?;
        let mut compiled = Vec::with_capacity(shaders.len());
        let mut result = Ok(());
        for &(shader_type, src) in shaders {
            let sh = match gl.create_shader(shader_type) {
                Ok(sh) => sh,
                Err(e) => {
                    result = Err(ProgramError::Create(e));
                    break;
                }
            };
            gl.shader_source(sh, src);
            gl.compile_shader(sh);
            compiled.push(sh);
            if !gl.get_shader_compile_status(sh) {
                result = Err(ProgramError::Compile {
                    shader_type,
                    log: gl.get_shader_info_log(sh),
                });
                break;
            }
            gl.attach_shader(program, sh);
        }
        if result.is_ok() {
            before_link(program);
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                result = Err(ProgramError::Link(gl.get_program_info_log(program)));
            }
        }
        // リンクした後のシェーダーは要らない（つないだままでも、プログラムを消すときに消える）。
        for sh in compiled {
            gl.delete_shader(sh);
        }
        if let Err(e) = result {
            gl.delete_program(program);
            return Err(e);
        }
        Ok(program)
    }
}
