#version 440
layout(location=0) in vec2 uv;
layout(location=0) out vec4 fragColor;
layout(binding=0, std140) uniform Canvas {
    vec4 currentMap;
    vec4 beforeMap;
    vec4 options; // source HDR, output encoding (0 SDR, 1 PQ, 2 scRGB), split, flip
    vec4 guide;
    vec4 tool; // shape, logical width, logical height, brush radius
    vec4 maskSegments[65]; // Crop pivot, or Y-axis handle followed by projected outline segments
};
layout(binding=1) uniform sampler2D currentImage;
layout(binding=2) uniform sampler2D beforeImage;
vec4 sampleFrame(sampler2D tex, vec4 mapping, vec2 point) {
    vec2 at = mapping.xy + point * mapping.zw;
    if (any(lessThan(at, vec2(0))) || any(greaterThan(at, vec2(1)))) return vec4(0);
    return texture(tex, at);
}
vec3 pq(vec3 linear) {
    vec3 p = pow(clamp(linear * (203.0/10000.0), 0.0, 1.0), vec3(2610.0/16384.0));
    return pow((3424.0/4096.0 + (2413.0/128.0)*p) / (1.0 + (2392.0/128.0)*p), vec3(2523.0/32.0));
}
void main() {
    vec2 point = vec2(uv.x, options.w > 0.5 ? uv.y : 1.0 - uv.y);
    vec2 samplePoint=point;
    if((tool.x==1.0 || tool.x==5.0 || tool.x==6.0) && abs(tool.w)>0.00001){
        vec2 center=maskSegments[0].xy;
        vec2 delta=(point-center)*tool.yz;
        float c=cos(tool.w),s=sin(tool.w);
        samplePoint=center+vec2(c*delta.x+s*delta.y,-s*delta.x+c*delta.y)/tool.yz;
    }
    vec4 c = point.x < options.z ? sampleFrame(beforeImage, beforeMap, point) : sampleFrame(currentImage, currentMap, samplePoint);
    vec3 rgb = c.rgb;
    if (options.x > 0.5) {
        rgb += vec3(0.010329823) * (1.0-c.a);
        if (options.y > 1.5) {
            // Rec.2020 -> extended linear sRGB; scRGB 1.0 corresponds to 80 nits.
            rgb = mat3(1.660491,-0.124550,-0.018151, -0.587641,1.132900,-0.100579, -0.072850,-0.008349,1.118730) * rgb * (203.0/80.0);
        } else rgb = pq(rgb);
    } else rgb += vec3(0.094117647) * (1.0-c.a);
    if (options.z >= 0.0 && abs(point.x-options.z) < 0.001) rgb = vec3(options.y > 0.5 ? 0.5 : 0.8);
    bool stroke = false;
    vec2 a=guide.xy*tool.yz, b=guide.zw*tool.yz, p=point*tool.yz;
    if (tool.x==1.0 || tool.x==5.0 || tool.x==6.0) {
        vec2 lo=min(a,b),hi=max(a,b);
        bool inside=all(greaterThanEqual(p,lo)) && all(lessThanEqual(p,hi));
        if (!inside) rgb*=0.4;
        stroke=inside && min(min(p.x-lo.x,hi.x-p.x),min(p.y-lo.y,hi.y-p.y))<1.2;
        vec2 thirds=(p-lo)/max(hi-lo,vec2(1));
        bool grid=inside&&(min(abs(thirds.x-1.0/3.0),abs(thirds.x-2.0/3.0))*(hi.x-lo.x)<0.5||min(abs(thirds.y-1.0/3.0),abs(thirds.y-2.0/3.0))*(hi.y-lo.y)<0.5);
        if(tool.x==5.0) grid=inside&&(abs(thirds.x-0.5)*(hi.x-lo.x)<0.5||abs(thirds.y-0.5)*(hi.y-lo.y)<0.5);
        if(tool.x==6.0) grid=false;
        if(grid)rgb=mix(rgb,vec3(0.85),0.4);
        vec2 edge=min(abs(p-lo),abs(p-hi));
        stroke=stroke||(min(edge.x,edge.y)<2.0&&max(edge.x,edge.y)<14.0);
        // Midpoint grips use logical pixels, like the pointer hit targets.
        vec2 mid=(lo+hi)*0.5;
        stroke=stroke||(abs(p.x-mid.x)<7.0&&edge.y<2.0)
                     ||(abs(p.y-mid.y)<7.0&&edge.x<2.0);
    } else if (tool.x==2.0) {
        float radius=max(length(b-a),tool.w);
        stroke=abs(length(p-a)-radius)<1.5;
    } else if (tool.x==3.0) {
        vec2 d=b-a;
        float lengthD=max(length(d),1.0);
        float along=dot(p-a,d)/lengthD;
        stroke=(abs(along)<0.6||abs(along-lengthD)<0.6)||length(p-(a+d*clamp(dot(p-a,d)/max(dot(d,d),0.001),0.0,1.0)))<0.6;
        stroke=stroke||abs(length(p-(a+b)*0.5)-6.0)<1.2||abs(length(p-a)-4.0)<1.2||abs(length(p-b)-4.0)<1.2;
    } else if(tool.x==4.0){
        vec2 radius=max(abs(b-a),vec2(4.0));
        stroke=abs(length((p-a)/radius)-1.0)*min(radius.x,radius.y)<1.2||abs(length(p-a)-6.0)<1.2;
    }
    if(tool.x==7.0) {
        for(int i=1;i<65;i++) {
            vec2 u=maskSegments[i].xy,v=maskSegments[i].zw,d=v-u;
            float t=clamp(dot(p-u,d)/max(dot(d,d),0.001),0.0,1.0);
            stroke=stroke||length(p-u-t*d)<0.8;
        }
        stroke=stroke||abs(length(p-a)-6.0)<1.2||abs(length(p-b)-4.0)<1.2||abs(length(p-maskSegments[0].xy)-4.0)<1.2;
    } else if(tool.x==8.0) stroke=abs(length(p-a)-6.0)<1.2;
    if (stroke) rgb=vec3(options.y>0.5?0.6:0.95);
    fragColor = vec4(rgb, 1.0);
}
