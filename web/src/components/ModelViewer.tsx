import { useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { bounds } from "../model3d";
import type { ModelObject } from "../model3d";

interface Props {
  objects: ModelObject[];
  /** What the canvas shows, for screen readers. */
  label: string;
}

/**
 * Renders a Print File's meshes with Three.js (ADR-0002): drag to orbit,
 * scroll or pinch to zoom, right-drag to pan. Each object is coloured by its
 * filament. 3MF is Z-up, so the model is turned to Three.js's Y-up.
 */
/** Whether this browser can draw WebGL at all. */
function webglAvailable(): boolean {
  try {
    const canvas = document.createElement("canvas");
    return Boolean(canvas.getContext("webgl2") ?? canvas.getContext("webgl"));
  } catch {
    return false;
  }
}

export default function ModelViewer({ objects, label }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const [failed] = useState<string | null>(() =>
    webglAvailable() ? null : "This browser can't show 3D (WebGL is off or unavailable).",
  );

  useEffect(() => {
    const element = host.current;
    if (!element) return;
    let renderer: THREE.WebGLRenderer;
    try {
      renderer = new THREE.WebGLRenderer({ antialias: true });
    } catch {
      element.textContent = "The 3D view couldn't start in this browser.";
      return;
    }
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    renderer.domElement.setAttribute("role", "img");
    renderer.domElement.setAttribute("aria-label", label);
    renderer.domElement.dataset.testid = "model-canvas";
    element.appendChild(renderer.domElement);

    const scene = new THREE.Scene();
    scene.background = new THREE.Color(getComputedStyle(element).getPropertyValue("--viewer-bg").trim() || "#f3f4f6");
    scene.add(new THREE.HemisphereLight(0xffffff, 0x666666, 2.2));
    const sun = new THREE.DirectionalLight(0xffffff, 1.6);
    sun.position.set(1, 2, 1.5);
    scene.add(sun);

    const model = new THREE.Group();
    model.rotation.x = -Math.PI / 2;
    const disposables: { dispose(): void }[] = [];
    for (const o of objects) {
      const geometry = new THREE.BufferGeometry();
      geometry.setAttribute("position", new THREE.BufferAttribute(o.positions, 3));
      geometry.setIndex(new THREE.BufferAttribute(o.indices, 1));
      geometry.computeVertexNormals();
      const material = new THREE.MeshStandardMaterial({ color: o.colour, roughness: 0.6, metalness: 0.05 });
      const mesh = new THREE.Mesh(geometry, material);
      mesh.name = o.name;
      model.add(mesh);
      disposables.push(geometry, material);
    }
    scene.add(model);

    // Frame the whole model, sitting on a grid like a bed.
    const { min, max } = bounds(objects);
    const size = Math.max(max[0] - min[0], max[1] - min[1], max[2] - min[2], 1);
    const centre = new THREE.Vector3((min[0] + max[0]) / 2, (min[2] + max[2]) / 2, -(min[1] + max[1]) / 2);
    const grid = new THREE.GridHelper(size * 2, 20, 0x999999, 0xcccccc);
    grid.position.set(centre.x, min[2], centre.z);
    scene.add(grid);
    disposables.push(grid.geometry, grid.material as THREE.Material);

    const camera = new THREE.PerspectiveCamera(40, 1, size / 100, size * 100);
    camera.position.set(centre.x + size * 1.2, centre.y + size * 1.0, centre.z + size * 1.6);
    const controls = new OrbitControls(camera, renderer.domElement);
    controls.target.copy(centre);
    controls.enableDamping = true;
    controls.minDistance = size * 0.2;
    controls.maxDistance = size * 20;
    controls.update();

    const resize = () => {
      const width = element.clientWidth || 600;
      const height = Math.max(280, Math.round(width * 0.6));
      renderer.setSize(width, height);
      camera.aspect = width / height;
      camera.updateProjectionMatrix();
    };
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(element);

    let frame = 0;
    const draw = () => {
      frame = requestAnimationFrame(draw);
      controls.update();
      renderer.render(scene, camera);
    };
    draw();

    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      controls.dispose();
      for (const d of disposables) d.dispose();
      renderer.dispose();
      renderer.domElement.remove();
    };
  }, [objects, label]);

  if (failed) return <p role="alert">{failed}</p>;
  return <div ref={host} className="model-viewer" />;
}
