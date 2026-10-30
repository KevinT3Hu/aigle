@group(0) @binding(0) var overview: texture_2d<f32>;
@group(0) @binding(1) var detail: texture_2d<f32>;
@group(0) @binding(2) var before_overview: texture_2d<f32>;
@group(0) @binding(3) var before_detail: texture_2d<f32>;
@group(0) @binding(4) var image_sampler: sampler;
struct Canvas { maps:array<vec4<f32>,4>, split:vec4<f32> };
@group(0) @binding(5) var<uniform> canvas:Canvas;
struct Vertex { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32> };
@vertex fn vs(@builtin(vertex_index) index:u32)->Vertex {
    let positions=array(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
    let p=positions[index];
    return Vertex(vec4(p,0.0,1.0),vec2((p.x+1.0)*0.5,(1.0-p.y)*0.5));
}
fn sample_image(image:texture_2d<f32>, uv:vec2<f32>, mapping:vec4<f32>)->vec4<f32> {
    let at=mapping.xy+uv*mapping.zw;
    if(any(at<vec2(0.0)) || any(at>vec2(1.0))) {return vec4(0.0);}
    // Transparent crop pixels are zero. Linear interpolation therefore supplies
    // premultiplied RGB at the crop edge, ready for linear-light composition.
    return textureSampleLevel(image,image_sampler,at,0.0);
}
@fragment fn fs(v:Vertex)->@location(0) vec4<f32> {
    var backing=sample_image(overview,v.uv,canvas.maps[0]);
    var foreground=sample_image(detail,v.uv,canvas.maps[1]);
    if(canvas.split.x>=0.0 && v.uv.x<canvas.split.x) {
        backing=sample_image(before_overview,v.uv,canvas.maps[2]);
        foreground=sample_image(before_detail,v.uv,canvas.maps[3]);
    }
    let background=vec3(0.010329823); // sRGB #1a1a1a in linear light.
    let base=backing.rgb+background*(1.0-backing.a);
    let rgb=clamp(foreground.rgb+base*(1.0-foreground.a),vec3(0.0),vec3(1000.0/203.0));
    let p=pow(rgb*(203.0/10000.0),vec3(2610.0/16384.0));
    let pq=pow((vec3(3424.0/4096.0)+(2413.0/128.0)*p)/(vec3(1.0)+(2392.0/128.0)*p),vec3(2523.0/32.0));
    return vec4(pq,1.0);
}
