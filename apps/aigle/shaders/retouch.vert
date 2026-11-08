#version 440
layout(location=0) in vec2 position;
layout(location=1) in vec4 color;
layout(location=0) out vec4 tint;
layout(binding=0,std140) uniform Overlay { vec4 viewport; };
void main() {
    vec2 p=position/viewport.xy*2.0-1.0;
    gl_Position=vec4(p.x,p.y*viewport.z,0.0,1.0);
    tint=color;
}
